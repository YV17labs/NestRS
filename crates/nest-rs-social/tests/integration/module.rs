//! `src/module.rs` — when `SocialModule` fills its registry: once, by the
//! kernel's wiring step, before the first lifecycle hook.

use std::sync::{Arc, Mutex};

use nest_rs_core::{App, hooks, injectable, module};
use nest_rs_social::{GithubSocialConfig, SocialModule, SocialRegistry};

/// The provider keys the hook saw.
#[injectable]
#[derive(Default)]
struct KeysAtInit {
    keys: Mutex<Vec<&'static str>>,
}

#[injectable]
struct LoginRoutes {
    #[inject]
    registry: Arc<SocialRegistry>,
    #[inject]
    seen: Arc<KeysAtInit>,
}

#[hooks]
impl LoginRoutes {
    #[on_module_init]
    async fn list_providers(&self) {
        *self.seen.keys.lock().expect("only this hook writes it") = self.registry.keys();
    }
}

#[module(imports = [SocialModule], providers = [KeysAtInit, LoginRoutes])]
struct LoginModule;

#[tokio::test]
async fn an_init_hook_reads_the_configured_providers() {
    let app = App::builder()
        .module::<LoginModule>()
        .provide(GithubSocialConfig {
            client_id: "seeded-client".into(),
            client_secret: "seeded-secret".into(),
            redirect_url: "https://acme.example.com/auth/github/callback".into(),
            scopes: Vec::new(),
        })
        .build()
        .await
        .expect("boots");
    app.init().await.expect("the init hooks run");

    let seen = app
        .container()
        .get::<KeysAtInit>()
        .expect("KeysAtInit is provided");
    assert_eq!(
        *seen.keys.lock().expect("the hook is done"),
        ["github"],
        "the registry was filled before the first hook",
    );
}
