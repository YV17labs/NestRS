//! The `#[cron_job]` decorator, re-exported by `nestrs-schedule`. The generated
//! code uses absolute paths (`::nestrs_schedule::*`, `::nestrs_core::*`,
//! `::std::*`), so this crate does not depend on them — they resolve at the call
//! site. Token-building helpers are shared with the other decorators via
//! `nestrs-codegen`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct, LitStr};

use nestrs_codegen::{
    build_injectable_body, from_container_method, injected_method, parse_named_str_arg,
    InjectableBody,
};

/// Mark a struct as a scheduled job that runs on a fixed interval.
///
/// `#[cron_job(every = "30s")]` on a struct that implements
/// [`Scheduled`](../nestrs_schedule/trait.Scheduled.html). Construction mirrors
/// `#[injectable]` — fields tagged `#[inject]` are resolved from the container,
/// others default, and the macro emits `from_container` — and additionally emits
/// `impl Discoverable` attaching a `CronJobMeta`: the job name, its period, and a
/// thunk that builds the job from the container and calls `Scheduled::run`. The
/// `Scheduler` transport discovers those metas at boot and ticks each one.
///
/// `every` accepts a number with an `ms`, `s`, `m`, or `h` suffix (`"500ms"`,
/// `"30s"`, `"5m"`, `"1h"`). The first run is one period after boot, then every
/// period.
#[proc_macro_attribute]
pub fn cron_job(args: TokenStream, input: TokenStream) -> TokenStream {
    let every = match parse_named_str_arg(args.into(), "every", "cron_job") {
        Ok(lit) => lit,
        Err(err) => return err.to_compile_error().into(),
    };
    let millis = match period_millis(&every) {
        Ok(ms) => ms,
        Err(err) => return err.to_compile_error().into(),
    };

    let mut item = parse_macro_input!(input as ItemStruct);
    let InjectableBody { ctor, dep_keys } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let name_lit = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let injected = injected_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #injected

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                builder.attach_meta::<Self, ::nestrs_schedule::CronJobMeta>(
                    ::nestrs_schedule::CronJobMeta {
                        name: #name_lit,
                        period: ::std::time::Duration::from_millis(#millis),
                        run: |__container| {
                            ::std::boxed::Box::pin(async move {
                                let __job = <Self>::from_container(__container);
                                ::nestrs_schedule::Scheduled::run(&__job).await
                            })
                        },
                    },
                )
            }
        }
    }
    .into()
}

/// The period in whole milliseconds for an `every = "<duration>"` literal,
/// parsed at macro-expansion time so a bad literal is a compile error rather than
/// a runtime surprise. Accepts an `ms`/`s`/`m`/`h` suffix.
fn period_millis(lit: &LitStr) -> syn::Result<u64> {
    let raw = lit.value();
    let s = raw.trim();
    let bad = || {
        syn::Error::new(
            lit.span(),
            "duration must be a positive number with an `ms`, `s`, `m`, or `h` suffix \
             (e.g. \"500ms\", \"30s\", \"5m\", \"1h\")",
        )
    };
    // `ms` before the single-char `s` so "500ms" is not mis-read as "500m".
    let (number, multiplier) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1u64)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        return Err(bad());
    };
    let value: u64 = number.trim().parse().map_err(|_| bad())?;
    if value == 0 {
        return Err(syn::Error::new(
            lit.span(),
            "duration must be greater than zero",
        ));
    }
    value
        .checked_mul(multiplier)
        .ok_or_else(|| syn::Error::new(lit.span(), "duration overflows u64 milliseconds"))
}
