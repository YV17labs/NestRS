//! A controller's path opens every route's template, so it reads the route
//! grammar: a 6.x parameter is refused with its 7.0 spelling, and a catch-all,
//! which its routes would follow, is refused too.

use nest_rs::http::controller;

#[controller(path = "/orgs/:org")]
pub struct MembersController;

#[controller(path = "/files/{*rest}")]
pub struct FilesController;

fn main() {}
