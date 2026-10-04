//! `imports` and `providers` each take a list, and a value of another kind is
//! refused at the value, opening with the decorator and the key: a bare type
//! where a list goes, at either key, and a provider entry that is not a type.
//! syn answered `expected square brackets` and `expected identifier`.

use nest_rs::core::module;

struct UsersService;
struct UsersModule;

#[module(providers = UsersService)]
struct AModule;

#[module(imports = UsersModule)]
struct BModule;

#[module(providers = ["UsersService"])]
struct CModule;

fn main() {}
