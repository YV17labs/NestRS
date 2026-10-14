//! [`ForInstance`] — what an instance-capable module's `for_root` takes: the
//! config pinned for one instance.

use std::marker::PhantomData;

use nest_rs_core::{Instance, Unnamed};

use crate::config::Config;

/// What an instance-capable module's `for_root` takes: the config pinned for
/// one instance, the default one unless a marker names another.
///
/// The default instance takes `x`, a named one `(Marker, x)` — `x` being
/// `None`, or a config the environment overlays per field:
///
/// ```
/// # use std::marker::PhantomData;
/// use nest_rs_config::{Config, ConfigModule, ConfigService, ConfigSetup, ForInstance, config};
/// use nest_rs_core::{
///     App, Collecting, Container, ContainerBuilder, Instance, Instanced, Module, Registering,
///     Unnamed, instance, module,
/// };
///
/// #[config(namespace = "bucket_doc")]
/// #[derive(Clone, Debug, Default)]
/// pub struct BucketConfig {
///     pub name: String,
/// }
/// # impl Config for BucketConfig {
/// #     fn from_env(_: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
/// #         Ok(base)
/// #     }
/// # }
///
/// /// A module that can be opened once per instance, written by hand.
/// pub struct BucketModule<I = Unnamed>(PhantomData<fn() -> I>);
///
/// impl<I: Instance> Module for BucketModule<I> {
///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
///         builder
///     }
///     fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
///         ConfigModule::provide_feature(ForInstance::<I, BucketConfig>::default(), builder)
///     }
/// }
///
/// impl BucketModule {
///     pub fn for_root<I: Instance>(
///         config: impl Into<ForInstance<I, BucketConfig>>,
///     ) -> ConfigSetup<BucketModule<I>, BucketConfig, I> {
///         ConfigModule::setup(config)
///     }
/// }
///
/// #[instance]
/// pub struct Media;
///
/// #[instance]
/// pub struct Thumbnails;
///
/// #[instance]
/// pub struct Avatars;
///
/// #[module(imports = [
///     BucketModule::for_root(None),
///     BucketModule::for_root((Media, None)),
///     BucketModule::for_root((Thumbnails, BucketConfig { name: "thumbs".into() })),
///     BucketModule::<Avatars>,
/// ])]
/// struct AppModule;
///
/// #[module(imports = [BucketModule::for_root(BucketConfig { name: "only".into() })])]
/// struct PinnedModule;
///
/// // Where the config type is itself inferred, or generic.
/// fn generic<C: Config>(config: Option<C>, builder: ContainerBuilder) -> ContainerBuilder {
///     ConfigModule::provide_feature(config, builder)
/// }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let _ = generic(None::<BucketConfig>, Container::builder());
/// # let _ = ConfigModule::provide_feature(None::<BucketConfig>, Container::builder());
/// let app = App::builder().module::<AppModule>().build().await?;
/// let thumbnails = app.container().get::<Instanced<Thumbnails, BucketConfig>>();
/// assert_eq!(thumbnails.map(|c| c.name.clone()).as_deref(), Some("thumbs"));
/// assert!(app.container().get::<Instanced<Media, BucketConfig>>().is_some());
/// assert!(app.container().get::<Instanced<Avatars, BucketConfig>>().is_some());
/// assert!(app.container().get::<BucketConfig>().is_some());
/// # let app = App::builder().module::<PinnedModule>().build().await?;
/// # assert_eq!(app.container().get::<BucketConfig>().map(|c| c.name.clone()).as_deref(), Some("only"));
/// # Ok(())
/// # }
/// ```
pub struct ForInstance<I, C> {
    pinned: Option<C>,
    instance: PhantomData<fn() -> I>,
}

impl<I, C> ForInstance<I, C> {
    fn pinning(pinned: Option<C>) -> Self {
        Self {
            pinned,
            instance: PhantomData,
        }
    }

    /// The config pinned in code, the base the environment overlays.
    pub(crate) fn into_pinned(self) -> Option<C> {
        self.pinned
    }
}

/// Nothing pinned: the instance reads its environment over its config's
/// defaults, as a bare import does.
impl<I, C> Default for ForInstance<I, C> {
    fn default() -> Self {
        Self::pinning(None)
    }
}

impl<I, C: Clone> Clone for ForInstance<I, C> {
    fn clone(&self) -> Self {
        Self::pinning(self.pinned.clone())
    }
}

impl<C: Config> From<Option<C>> for ForInstance<Unnamed, C> {
    fn from(pinned: Option<C>) -> Self {
        Self::pinning(pinned)
    }
}

impl<C: Config> From<C> for ForInstance<Unnamed, C> {
    fn from(pinned: C) -> Self {
        Self::pinning(Some(pinned))
    }
}

impl<I: Instance, C: Config> From<(I, Option<C>)> for ForInstance<I, C> {
    fn from((_, pinned): (I, Option<C>)) -> Self {
        Self::pinning(pinned)
    }
}

impl<I: Instance, C: Config> From<(I, C)> for ForInstance<I, C> {
    fn from((_, pinned): (I, C)) -> Self {
        Self::pinning(Some(pinned))
    }
}
