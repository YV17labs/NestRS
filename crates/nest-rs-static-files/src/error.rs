//! [`StaticFilesError`] — why the boot refused the files a
//! [`StaticFilesModule`](crate::StaticFilesModule) was given: each names every
//! way to set the directory, never its value.

use crate::config::StaticFilesConfig;

/// Why the boot refused the files a
/// [`StaticFilesModule`](crate::StaticFilesModule) was given.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StaticFilesError {
    /// No directory named, and no [`Embedded`](crate::Embedded) source.
    #[error(
        "StaticFilesModule serves a directory and none is named — set {}, or import \
         StaticFilesModule::for_root(Embedded::of::<T>()) for a folder embedded in the binary",
        root_setting()
    )]
    RootUnset,
    /// The directory does not resolve — missing, or out of the process's reach.
    #[error("{}, names a directory that does not resolve", root_setting())]
    RootUnresolved {
        /// What resolving it answered.
        #[source]
        source: std::io::Error,
    },
    /// The root resolves to something other than a directory.
    #[error("{}, names something that is not a directory", root_setting())]
    RootNotADirectory,
    /// A directory named beside a folder embedded in the binary.
    #[error(
        "{}, is set, and StaticFilesModule serves {embedded}, a folder embedded in the binary: \
         a source is one or the other — unset the directory, or drop the embedded source to \
         serve it",
        root_setting()
    )]
    RootBesideEmbedded {
        /// The type deriving `Embed` that names the embedded folder.
        embedded: &'static str,
    },
}

/// Every way [`StaticFilesConfig::root`] can be given, for a refusal to name.
fn root_setting() -> String {
    format!(
        "{}, or `StaticFilesConfig::root` in code",
        nest_rs_config::spellings(
            <StaticFilesConfig as nest_rs_config::Namespaced>::NAMESPACE,
            "ROOT"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_names_the_variable_and_the_field() {
        for err in [
            StaticFilesError::RootUnset,
            StaticFilesError::RootNotADirectory,
            StaticFilesError::RootBesideEmbedded { embedded: "Site" },
        ] {
            let said = err.to_string();
            assert!(
                said.contains(&nest_rs_config::var_name("static_files", "ROOT")),
                "{said}"
            );
            assert!(said.contains("StaticFilesConfig::root"), "{said}");
        }
    }
}
