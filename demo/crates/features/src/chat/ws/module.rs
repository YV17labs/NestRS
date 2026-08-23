use nest_rs::core::module;
use nest_rs::ws::WsModule;

use super::gateway::ChatGateway;
use super::guard::ModerationGuard;
use super::request_seq::RequestSeq;
use super::seq_source::SeqSource;
use crate::authn::AuthnModule;
use crate::chat::ChatModule;

#[module(
    imports = [ChatModule, AuthnModule, WsModule],
    providers = [ChatGateway, ModerationGuard, SeqSource, RequestSeq],
)]
pub struct ChatWsModule;
