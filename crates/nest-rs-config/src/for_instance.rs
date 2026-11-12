//! [`ForInstance`] — what an instance-capable module's `for_root` takes: the
//! config pinned for one instance.

use std::marker::PhantomData;

use nest_rs_core::{Instance, Unnamed};

use crate::config::Config;

/// What an instance-capable module's `for_root` takes: the config pinned for
/// one instance, the default one unless a marker names another.
///
/// The default instance takes `x`, a named one `(Marker, x)` — `x` a config
/// the environment overlays, per field. Nothing pinned is the bare import,
/// `BucketModule` or `BucketModule::<Media>`, never a value:
///
/// ```
/// # use std::marker::PhantomData;
/// use nest_rs_config::{ConfigModule, ConfigSetup, ForInstance, config};
/// use nest_rs_core::{
///     App, Collecting, ContainerBuilder, Instance, Instanced, Module, Registering, Unnamed,
///     instance, module,
/// };
///
/// #[config(namespace = "bucket_doc")]
/// #[derive(Clone, Debug, Default)]
/// pub struct BucketConfig {
///     pub name: String,
/// }
///
/// /// A module that can be opened once per instance, written by hand.
/// pub struct BucketModule<I = Unnamed>(PhantomData<fn() -> I>);
///
/// impl<I: Instance> Module for BucketModule<I> {
///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
///         builder
///     }
///     fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
///         ConfigModule::provide_feature::<I, BucketConfig>(builder)
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
/// #[module(imports = [
///     BucketModule,
///     BucketModule::<Media>,
///     BucketModule::for_root((Thumbnails, BucketConfig { name: "thumbs".into() })),
/// ])]
/// struct AppModule;
///
/// #[module(imports = [BucketModule::for_root(BucketConfig { name: "only".into() })])]
/// struct PinnedModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let app = App::builder().module::<AppModule>().build().await?;
/// let thumbnails = app.container().get::<Instanced<Thumbnails, BucketConfig>>();
/// assert_eq!(thumbnails.map(|c| c.name.clone()).as_deref(), Some("thumbs"));
/// assert!(app.container().get::<Instanced<Media, BucketConfig>>().is_some());
/// assert!(app.container().get::<BucketConfig>().is_some());
/// # let app = App::builder().module::<PinnedModule>().build().await?;
/// # assert_eq!(app.container().get::<BucketConfig>().map(|c| c.name.clone()).as_deref(), Some("only"));
/// # Ok(())
/// # }
/// ```
pub struct ForInstance<I, C> {
    pinned: C,
    instance: PhantomData<fn() -> I>,
}

impl<I, C> ForInstance<I, C> {
    fn pinning(pinned: C) -> Self {
        Self {
            pinned,
            instance: PhantomData,
        }
    }

    /// The config pinned in code, the base the environment overlays.
    pub(crate) fn into_pinned(self) -> C {
        self.pinned
    }
}

impl<I, C: Clone> Clone for ForInstance<I, C> {
    fn clone(&self) -> Self {
        Self::pinning(self.pinned.clone())
    }
}

impl<C: Config> From<C> for ForInstance<Unnamed, C> {
    fn from(pinned: C) -> Self {
        Self::pinning(pinned)
    }
}

impl<I: Instance, C: Config> From<(I, C)> for ForInstance<I, C> {
    fn from((_, pinned): (I, C)) -> Self {
        Self::pinning(pinned)
    }
}
