//! Listing a type implementing no `ExceptionFilter` as one names the trait and
//! what to implement, never rustc's bare unsatisfied bound.

use nest_rs::core::{GlobalExceptionFilters, Layer, injectable, module};

#[injectable]
#[derive(Default)]
struct Forgetful;

impl Layer for Forgetful {}

#[module(providers = [Forgetful, GlobalExceptionFilters<(Forgetful,)>])]
struct AppModule;

fn main() {}
