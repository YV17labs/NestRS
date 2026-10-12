//! Postgres savepoints are a stack on one connection: a statement a unit issues
//! while one of its atomic blocks is open lands inside the block's savepoint
//! and rolls back with it. So each unit hands out one turn at a time, and an
//! open block keeps it from its `SAVEPOINT` until it settles.

use std::sync::atomic::{AtomicUsize, Ordering};

use sea_orm::DbErr;
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::executor::Executor;

/// What a statement issued on a unit from inside the very block that holds
/// that unit's turn is refused with: waiting would wait for itself.
const WAITS_FOR_ITSELF: &str = "a statement on a unit of work was issued from inside one of its own \
     atomic blocks, which holds that unit until it settles; run it on the block, through the \
     ambient executor";

/// One statement at a time on a unit, or one atomic block for as long as it is
/// open.
pub(crate) struct Turn {
    permits: Semaphore,
    /// The address of the block holding the turn, `0` when none does.
    block: AtomicUsize,
}

impl Turn {
    pub(crate) fn new() -> Self {
        Self {
            permits: Semaphore::new(1),
            block: AtomicUsize::new(0),
        }
    }

    /// The turn for one statement of the unit itself. Uncontended, it costs one
    /// atomic operation; a statement issued inside the block holding it is
    /// refused rather than left to wait for itself.
    pub(crate) async fn for_statement(&self) -> Result<SemaphorePermit<'_>, DbErr> {
        if let Ok(permit) = self.permits.try_acquire() {
            return Ok(permit);
        }
        let holder = self.block.load(Ordering::Acquire);
        if holder != 0 && inside_block(holder) {
            return Err(DbErr::Custom(WAITS_FOR_ITSELF.to_owned()));
        }
        self.permits
            .acquire()
            .await
            .map_err(|closed| DbErr::Custom(closed.to_string()))
    }

    /// The turn for the block at `block`, kept until [`give_back`](Self::give_back).
    pub(crate) async fn for_block(&self, block: usize) -> Result<(), DbErr> {
        let permit = self.for_statement().await?;
        permit.forget();
        self.block.store(block, Ordering::Release);
        Ok(())
    }

    /// Hand back the turn a block kept.
    pub(crate) fn give_back(&self) {
        self.block.store(0, Ordering::Release);
        self.permits.add_permits(1);
    }
}

/// Whether the ambient executor is the block at `holder`, or a block nested in
/// it.
fn inside_block(holder: usize) -> bool {
    let Some(ambient) = nest_rs_database::current_executor() else {
        return false;
    };
    let Some(Executor::Savepoint(block)) = ambient.as_any().downcast_ref::<Executor>() else {
        return false;
    };
    block.lineage().any(|block| block == holder)
}
