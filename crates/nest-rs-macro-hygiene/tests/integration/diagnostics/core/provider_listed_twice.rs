//! An entry written twice in one `providers` list registers one type twice;
//! it is refused at the second entry, whether it is bare or bound `as dyn`.
//! Two traits for one provider are not a repeat: `Foo as dyn A, Foo as dyn B`
//! holds one `Foo` under both.

use nest_rs::core::{injectable, module};

trait Bridge: Send + Sync {}

#[injectable]
#[derive(Default)]
struct Host;

impl Bridge for Host {}

#[module(providers = [Host, Host])]
struct BareTwiceModule;

#[module(providers = [Host as dyn Bridge, Host as dyn Bridge])]
struct BoundTwiceModule;

fn main() {}
