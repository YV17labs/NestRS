//! `src/query.rs`: `Query<T>` through a mounted route, answering what 6.x's
//! did, through the same edge.

use nest_rs_core::module;
use nest_rs_http::{FromRequest, IntoResponse, Method, controller, routes};
use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
struct Page {
    limit: u32,
    order: Option<Order>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Order {
    Asc,
    Desc,
}

fn said(page: &Page) -> String {
    let order = match page.order {
        Some(Order::Asc) => "asc",
        Some(Order::Desc) => "desc",
        None => "-",
    };
    format!("{} {order}", page.limit)
}

#[controller(path = "/six")]
struct SixController;

#[routes]
impl SixController {
    #[get("/query")]
    async fn query(&self, page: nest_rs_http::Query<Page>) -> String {
        said(&page)
    }

    #[get("/form")]
    async fn form(&self, page: nest_rs_http::Form<Page>) -> String {
        said(&page)
    }
}

#[module(providers = [SixController])]
struct SixModule;

#[tokio::test]
async fn query_and_form_refusals_answer_the_same_status_and_redacted_detail_as_6x() {
    let six = crate::boot::<SixModule>().await;
    let query = crate::vocabulary_route(Method::GET, "/six/query", |req, mut body| async move {
        let page = nest_rs_http::Query::<Page>::from_request(&req, &mut body).await?;
        Ok(said(&page).into_response())
    })
    .await;
    let form = crate::vocabulary_route(Method::GET, "/six/form", |req, mut body| async move {
        let page = nest_rs_http::Form::<Page>::from_request(&req, &mut body).await?;
        Ok(said(&page).into_response())
    })
    .await;

    for (seven, base) in [(&query, "/six/query"), (&form, "/six/form")] {
        for query in [
            "?limit=20",
            "?limit=20&order=desc",
            "",
            "?limit=sk_live_51",
            "?limit=5&order=sk_live_51",
            "?limit=5&limit=6",
            "?limit=%ZZ",
        ] {
            let path = format!("{base}{query}");
            let answer = crate::answered(seven.get(&path).send().await).await;
            assert!(!answer.2.contains("sk_live"), "{path}: {answer:?}");
            assert_eq!(
                answer,
                crate::answered(six.get(&path).send().await).await,
                "{path}"
            );
        }
    }
}
