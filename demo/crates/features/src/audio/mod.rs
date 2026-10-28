pub const TARGET: &str = "features::audio";

mod command;
mod config;
mod dtos;
mod error;
mod module;
mod service;
mod state;
mod transcodes;
mod transcoding;

pub mod http;
pub mod mcp;
pub mod queue;
pub mod schedule;

pub use command::{AUDIO_QUEUE, AudioQueue, TranscodeCommand};
pub use config::AudioConfig;
pub use dtos::{
    DirectUploadDto, PresignedUrlDto, StreamResumeDto, TranscodeDto, TranscodeProgressDto,
};
pub use module::AudioModule;
pub use service::AudioService;
pub use state::TranscodeState;
pub use transcodes::Transcodes;
pub use transcoding::Transcoding;

pub use http::AudioHttpModule;
pub use mcp::AudioMcpModule;
pub use queue::AudioQueueModule;
pub use schedule::AudioScheduleModule;
