pub mod win_network;

pub use win_network::{start_network_watcher, NetworkEvent, NetworkWatcherHandle, WatcherError};
