//! `Foo as dyn Trait` already holds `Foo` under its own type, so a bare `Foo`
//! beside it would be a second registration of one type. It was two instances:
//! the decorated methods fired on one while every `Arc<dyn Trait>` consumer held
//! the other. The list is refused at the second entry, in either order.

use nest_rs::core::{injectable, module};

trait Bridge: Send + Sync {}

#[injectable]
#[derive(Default)]
struct Host;

impl Bridge for Host {}

#[module(providers = [Host, Host as dyn Bridge])]
struct BareFirstModule;

#[module(providers = [Host as dyn Bridge, Host])]
struct BareSecondModule;

fn main() {}
