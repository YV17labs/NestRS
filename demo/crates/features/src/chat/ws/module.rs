use nest_rs::core::module;
use nest_rs::http::HttpModule;
use nest_rs::ws::WsModule;

use super::gateway::ChatGateway;
use super::request_seq::RequestSeq;
use super::seq_source::SeqSource;
use crate::authn::AuthnModule;
use crate::authz::AuthzModule;
use crate::chat::ChatModule;

#[module(
    imports = [ChatModule, AuthnModule, AuthzModule, HttpModule, WsModule],
    providers = [ChatGateway, SeqSource, RequestSeq],
)]
pub struct ChatWsModule;
