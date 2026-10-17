//! Postgres savepoints are a stack on one connection: a statement a unit issues
//! while one of its atomic blocks is open lands inside the block's savepoint
//! and rolls back with it. So each unit hands out one turn at a time, and an
//! open block keeps it from its `SAVEPOINT` until it settles.

use std::sync::atomic::{AtomicUsize, Ordering};

use sea_orm::DbErr;
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::executor::Executor;

/// What a statement issued on a unit from inside one of that unit's own blocks
/// is refused with: the block holds the unit's turn, or will once it opens, so
/// waiting would wait for itself.
const WAITS_FOR_ITSELF: &str = "a statement on a unit of work was issued from inside one of its own \
     atomic blocks, which holds that unit until it settles; run it on the block, through the \
     ambient executor";

/// One statement at a time on a unit, or one atomic block for as long as it is
/// open.
pub(crate) struct Turn {
    permits: Semaphore,
    /// How many atomic blocks sit in the unit, open or not yet.
    blocks: AtomicUsize,
}

impl Turn {
    pub(crate) fn new() -> Self {
        Self {
            permits: Semaphore::new(1),
            blocks: AtomicUsize::new(0),
        }
    }

    /// Count a block sitting in the unit, until [`leave`](Self::leave).
    pub(crate) fn enter(&self) {
        self.blocks.fetch_add(1, Ordering::Release);
    }

    /// Stop counting a block [`enter`](Self::enter) counted.
    pub(crate) fn leave(&self) {
        self.blocks.fetch_sub(1, Ordering::Release);
    }

    /// The turn for one statement of the unit itself. A statement issued from
    /// inside one of the unit's own blocks is refused, whether or not that
    /// block has opened yet. With no block in the unit, it reads nothing
    /// ambient; uncontended, it takes the permit without waiting and gives it
    /// back under the semaphore's lock.
    pub(crate) async fn for_statement(&self) -> Result<SemaphorePermit<'_>, DbErr> {
        if self.blocks.load(Ordering::Acquire) != 0 && issued_inside_a_block_of(self) {
            return Err(DbErr::Custom(WAITS_FOR_ITSELF.to_owned()));
        }
        self.take().await
    }

    /// The turn for a block of the unit, kept until [`give_back`](Self::give_back).
    pub(crate) async fn for_block(&self) -> Result<(), DbErr> {
        self.take().await?.forget();
        Ok(())
    }

    /// The turn for settling the block that owns this turn: every statement on
    /// it has finished, and none starts while the permit is held.
    pub(crate) async fn for_settling(&self) -> Result<SemaphorePermit<'_>, DbErr> {
        self.take().await
    }

    /// Hand back the turn a block kept.
    pub(crate) fn give_back(&self) {
        self.permits.add_permits(1);
    }

    async fn take(&self) -> Result<SemaphorePermit<'_>, DbErr> {
        if let Ok(permit) = self.permits.try_acquire() {
            return Ok(permit);
        }
        self.permits
            .acquire()
            .await
            .map_err(|closed| DbErr::Custom(closed.to_string()))
    }
}

/// Whether the ambient executor is a block sitting in the unit whose turn is
/// `turn`, or nested in one that does.
fn issued_inside_a_block_of(turn: &Turn) -> bool {
    let Some(ambient) = nest_rs_database::current_executor() else {
        return false;
    };
    let Some(Executor::Savepoint(block)) = ambient.as_any().downcast_ref::<Executor>() else {
        return false;
    };
    block.sits_in(turn)
}
