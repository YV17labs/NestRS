//! The indicator each instance `RedisModule::for_root` opens attaches, in
//! process: [`indicator`] for each instance answering the probes under its
//! own name, against scripted servers.

mod indicator;
