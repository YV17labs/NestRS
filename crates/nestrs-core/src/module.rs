use crate::container::ContainerBuilder;

/// A `Module` declares its providers by extending the container builder.
///
/// This is the rough equivalent of NestJS's `@Module({ providers: [...] })`.
/// Modules compose by chaining their `register` calls.
pub trait Module {
    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}
