use std::sync::Arc;

use nest_rs::http::futures_util::stream;
use nest_rs::http::{Body, Bytes, HeaderValue, Json, Response, controller, routes};

use crate::dto::{HelloDto, ItemDto};
use crate::guard::BearerGuard;
use crate::interceptor::PassInterceptor;
use crate::service::HelloService;

#[controller(path = "/")]
pub struct HelloController {
    #[inject]
    svc: Arc<HelloService>,
}

#[routes]
impl HelloController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    #[get("/hello")]
    async fn hello(&self) -> Json<HelloDto> {
        Json(HelloDto {
            message: self.svc.greeting(),
        })
    }

    #[get("/layered")]
    #[use_guards(BearerGuard)]
    #[use_interceptors(PassInterceptor)]
    async fn layered(&self) -> Json<HelloDto> {
        Json(HelloDto {
            message: self.svc.greeting(),
        })
    }

    #[get("/stream")]
    async fn stream(&self) -> Response {
        let chunks =
            stream::iter((0..16).map(|_| Ok::<_, std::io::Error>(Bytes::from_static(b"chunk\n"))));
        Response::builder()
            .content_type(HeaderValue::from_static("text/plain"))
            .body(Body::from_stream(chunks))
    }

    #[post("/echo")]
    async fn echo(&self, body: Json<HelloDto>) -> Json<HelloDto> {
        body
    }

    #[get("/large")]
    async fn large(&self) -> Json<Vec<ItemDto>> {
        Json(self.svc.items())
    }
}
