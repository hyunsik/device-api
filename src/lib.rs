//! A set of APIs to list and retrieve information of FuriosaAI's NPU devices.
//! To learn more about FuriosaAI's NPU, please visit <https://furiosa.ai>.
//!
//! # Before you start
//!
//! This crate requires FuriosaAI's NPU device and its kernel driver. Currently, FuriosaAI offers
//! NPU devices for only users who register Early Access Program (EAP). Please contact
//! <contact@furiosa.ai> to learn how to start the EAP. You can also refer to
//! [Driver, Firmware, and Runtime Installation](https://furiosa-ai.github.io/docs/latest/en/software/installation.html)
//! to learn the kernel driver installation.
//!
//! # Usage
//!
//! Add this to your 'Cargo.toml':
//! ```toml
//! [dependencies]
//! furiosa-device = "0.1"
//! ```
//!
//! ## Listing devices from the system
//!
//! The current implementation mainly offers two APIs, namely
//! [`list_devices`] and [`find_devices`].
//!
//! 1. [`list_devices`] enumerates all Furiosa NPU devices in the system.
//! One can simply call as below:
//! ```rust,ignore
//! let devices = furiosa_device::list_devices().await?;
//! ```
//!
//! [Struct `Device`][`Device`] offers methods for further information of each
//! device.
//!
//! 2. If you have a desired configuration, call [`find_devices`] with your device configuration
//! described by a [`DeviceConfig`]. [`find_devices`] will return a list of
//! [`DeviceFile`]s if there are matched devices.
//! ```rust,ignore
//! use furiosa_device::{DeviceConfig, find_devices};
//!
//! // Find two Warboy devices, fused.
//! let config = DeviceConfig::warboy().fused().count(2);
//! let dev_files = find_devices(&config).await?;
//! ```
//!
//! 3. In case you have prior knowledge on the system and want to pick out a
//! device with specific name, use [`get_device`].
//! ```rust,ignore
//! let device = furiosa_device::get_device("npu0pe0").await?;
//! ```

// Allows displaying feature flags in the documentation.
#![cfg_attr(docsrs, feature(doc_cfg))]

pub use crate::arch::Arch;
pub use crate::device::{CoreStatus, Device, DeviceFile, DeviceMode};
pub use crate::error::{DeviceError, DeviceResult};
use crate::find::{expand_status, find_devices_in};
pub use crate::find::{DeviceConfig, DeviceConfigBuilder};
use crate::list::list_devices_with;

mod arch;
#[cfg(feature = "blocking")]
#[cfg_attr(docsrs, doc(cfg(feature = "blocking")))]
pub mod blocking;
mod devfs;
mod device;
mod error;
mod find;
pub mod hwmon;
mod list;
mod status;
mod sysfs;

/// List all Furiosa NPU devices in the system.
///
/// See the [crate-level documentation](crate).
pub async fn list_devices() -> DeviceResult<Vec<Device>> {
    list_devices_with("/dev", "/sys").await
}

/// Find a set of devices with specific configuration.
///
/// # Arguments
///
/// * `config` - DeviceConfig
///
/// See the [crate-level documentation](crate).
pub async fn find_devices(config: &DeviceConfig) -> DeviceResult<Vec<DeviceFile>> {
    let devices = expand_status(list_devices().await?).await?;
    find_devices_in(config, &devices)
}

/// Return a specific device if it exists.
///
/// # Arguments
///
/// * `device_name` - A device name (e.g., npu0, npu0pe0, npu0pe0-1)
///
/// See the [crate-level documentation](crate).
pub async fn get_device<S: AsRef<str>>(device_name: S) -> DeviceResult<DeviceFile> {
    get_device_with("/dev", device_name.as_ref()).await
}

pub(crate) async fn get_device_with(devfs: &str, device_name: &str) -> DeviceResult<DeviceFile> {
    let path = devfs::path(devfs, device_name);
    if !path.exists() {
        return Err(DeviceError::DeviceNotFound {
            name: device_name.to_string(),
        });
    }

    let file = tokio::fs::File::open(&path).await?;
    if !devfs::is_character_device(file.metadata().await?.file_type()) {
        return Err(DeviceError::invalid_device_file(path.display()));
    }

    devfs::parse_indices(path.file_name().expect("not a file").to_string_lossy())?;

    DeviceFile::try_from(&path)
}
