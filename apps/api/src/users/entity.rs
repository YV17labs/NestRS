/// Domain entity — equivalent of `user.entity.ts` in NestJS.
/// Will be replaced by a sea-orm entity once persistence is added.
#[derive(Debug, Clone)]
pub struct User {
    pub id: u32,
    pub name: String,
    pub email: String,
}
