use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::str::FromStr;

use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::digit1;
use nom::combinator::{all_consuming, map, map_res, opt};
use nom::sequence::{delimited, separated_pair};
use nom::Parser;

use crate::arch::Arch;
use crate::device::{CoreIdx, CoreStatus, Device, DeviceFile, DeviceMode};
use crate::error::DeviceResult;

/// Describes a required set of devices for [`find_devices`][crate::find_devices].
///
/// # Examples
/// ```rust
/// use furiosa_device::DeviceConfig;
///
/// // 1 core
/// DeviceConfig::warboy().build();
///
/// // 1 core x 2
/// DeviceConfig::warboy().count(2);
///
/// // Fused 2 cores x 2
/// DeviceConfig::warboy().fused().count(2);
/// ```
///
/// See also [struct `Device`][`Device`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DeviceConfig {
    Named {
        device_id: u8,
        core_id: CoreId,
    },
    Unnamed {
        arch: Arch,
        mode: DeviceMode,
        count: u8,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CoreId {
    Id(u8),
    Range(u8, u8),
}

impl DeviceConfig {
    /// Returns a builder associated with Warboy NPUs.
    pub fn warboy() -> WarboyConfigBuilder {
        WarboyConfigBuilder {
            arch: Arch::Warboy,
            mode: DeviceMode::Single,
            count: 1,
        }
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig::warboy().fused().count(1)
    }
}

impl FromStr for DeviceConfig {
    type Err = nom::Err<()>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // try parsing named configs, from patterns e.g., "0:0" or "0:0-1"
        let parsed_named = all_consuming::<_, _, (), _>(separated_pair(
            map_res(digit1, |s: &str| s.parse::<u8>()),
            tag(":"),
            alt((
                map(
                    separated_pair(
                        map_res(digit1, |s: &str| s.parse::<u8>()),
                        tag("-"),
                        map_res(digit1, |s: &str| s.parse::<u8>()),
                    ),
                    |(s, e)| CoreId::Range(s, e),
                ),
                map(map_res(digit1, |s: &str| s.parse::<u8>()), |idx| {
                    CoreId::Id(idx)
                }),
            )),
        ))(s);

        match parsed_named {
            Ok((_, (device_id, core_id))) => Ok(DeviceConfig::Named { device_id, core_id }),
            Err(_) => {
                // try parsing unnamed configs, from patterns e.g., "warboy*1" or "warboy(1)*2"
                let (_, ((arch, mode), count)) = all_consuming(separated_pair(
                    map_res(tag("warboy"), |s: &str| s.parse::<Arch>()).and(opt(delimited(
                        tag("("),
                        map_res(digit1, |s: &str| s.parse::<u8>()),
                        tag(")"),
                    ))),
                    tag("*"),
                    map_res(digit1, |s: &str| s.parse::<u8>()),
                ))(s)?;
                let mode = match mode {
                    None => Ok(DeviceMode::MultiCore),
                    Some(1) => Ok(DeviceMode::Single),
                    // TODO: Improve below
                    Some(2) => Ok(DeviceMode::Fusion),
                    _ => Err(Self::Err::Failure(())),
                }?;

                Ok(DeviceConfig::Unnamed { arch, mode, count })
            }
        }
    }
}

/// A builder struct for `DeviceConfig` with Warboy NPUs.
pub struct WarboyConfigBuilder {
    arch: Arch,
    mode: DeviceMode,
    count: u8,
}

impl WarboyConfigBuilder {
    pub fn multicore(mut self) -> Self {
        self.mode = DeviceMode::MultiCore;
        self
    }

    pub fn fused(mut self) -> Self {
        self.mode = DeviceMode::Fusion;
        self
    }

    pub fn count(mut self, count: u8) -> DeviceConfig {
        self.count = count;
        DeviceConfig::Unnamed {
            arch: self.arch,
            mode: self.mode,
            count: self.count,
        }
    }

    pub fn build(self) -> DeviceConfig {
        DeviceConfig::Unnamed {
            arch: self.arch,
            mode: self.mode,
            count: self.count,
        }
    }
}

pub(crate) struct DeviceWithStatus {
    pub device: Device,
    pub statuses: HashMap<CoreIdx, CoreStatus>,
}

impl Deref for DeviceWithStatus {
    type Target = Device;

    fn deref(&self) -> &Self::Target {
        &self.device
    }
}

pub(crate) async fn expand_status(devices: Vec<Device>) -> DeviceResult<Vec<DeviceWithStatus>> {
    let mut new_devices = Vec::with_capacity(devices.len());
    for device in devices.into_iter() {
        new_devices.push(DeviceWithStatus {
            statuses: device.get_status_all().await?,
            device,
        })
    }
    Ok(new_devices)
}

pub(crate) fn find_devices_in(
    config: &DeviceConfig,
    devices: &[DeviceWithStatus],
) -> DeviceResult<Vec<DeviceFile>> {
    let mut allocated: HashMap<u8, HashSet<u8>> = HashMap::with_capacity(devices.len());

    for device in devices {
        allocated.insert(
            device.device_index(),
            device
                .statuses
                .iter()
                .filter(|(_, status)| **status != CoreStatus::Available)
                .map(|(core, _)| *core)
                .collect(),
        );
    }

    let (&config_arch, &config_mode, &config_count) = match config {
        // TODO: implementation
        DeviceConfig::Named {
            device_id: _,
            core_id: _,
        } => unimplemented!(),
        DeviceConfig::Unnamed { arch, mode, count } => (arch, mode, count),
    };

    let mut found: Vec<DeviceFile> = Vec::with_capacity(config_count.into());

    'outer: for _ in 0..config_count {
        for device in devices {
            if config_arch != device.arch() {
                continue;
            }
            // early exit for multicore
            if config_mode == DeviceMode::MultiCore
                && !allocated.get(&device.device_index()).unwrap().is_empty()
            {
                continue;
            }

            'inner: for dev_file in device
                .dev_files()
                .iter()
                .filter(|d| d.mode() == config_mode)
            {
                for idx in dev_file.core_indices() {
                    if allocated.get(&device.device_index()).unwrap().contains(idx) {
                        continue 'inner;
                    }
                }
                // this dev_file is suitable
                found.push(dev_file.clone());

                let used = allocated.get_mut(&device.device_index()).unwrap();
                used.extend(dev_file.core_indices());
                if dev_file.is_multicore() {
                    used.extend(device.cores());
                }
                continue 'outer;
            }
        }
        return Ok(vec![]);
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use crate::list::list_devices_with;

    use super::*;

    #[tokio::test]
    async fn test_find_devices() -> DeviceResult<()> {
        // test directory contains 2 warboy NPUs
        let devices = list_devices_with("test_data/test-0/dev", "test_data/test-0/sys").await?;
        let devices_with_statuses = expand_status(devices).await?;

        // try lookup 4 different single cores
        let config = DeviceConfig::warboy().count(4);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].filename(), "npu0pe0");
        assert_eq!(found[1].filename(), "npu0pe1");
        assert_eq!(found[2].filename(), "npu1pe0");
        assert_eq!(found[3].filename(), "npu1pe1");

        // looking for 5 different cores should fail
        let config = DeviceConfig::warboy().count(5);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found, vec![]);

        // try lookup 2 different fused cores
        let config = DeviceConfig::warboy().fused().count(2);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].filename(), "npu0pe0-1");
        assert_eq!(found[1].filename(), "npu1pe0-1");

        // looking for 3 different fused cores should fail
        let config = DeviceConfig::warboy().fused().count(3);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found, vec![]);

        Ok(())
    }

    #[test]
    fn test_config_from_named_text_repr() -> Result<(), nom::Err<()>> {
        assert!("0".parse::<DeviceConfig>().is_err());
        assert!("0:".parse::<DeviceConfig>().is_err());
        assert!(":0".parse::<DeviceConfig>().is_err());
        assert!("0:0-1-".parse::<DeviceConfig>().is_err());

        assert_eq!(
            "0:0".parse::<DeviceConfig>(),
            Ok(DeviceConfig::Named {
                device_id: 0,
                core_id: CoreId::Id(0)
            })
        );
        assert_eq!(
            "0:1".parse::<DeviceConfig>(),
            Ok(DeviceConfig::Named {
                device_id: 0,
                core_id: CoreId::Id(1)
            })
        );
        assert_eq!(
            "1:1".parse::<DeviceConfig>(),
            Ok(DeviceConfig::Named {
                device_id: 1,
                core_id: CoreId::Id(1)
            })
        );
        assert_eq!(
            "0:0-1".parse::<DeviceConfig>(),
            Ok(DeviceConfig::Named {
                device_id: 0,
                core_id: CoreId::Range(0, 1)
            })
        );

        Ok(())
    }

    #[test]
    fn test_config_from_unnamed_text_repr() -> Result<(), nom::Err<()>> {
        assert!("warboy".parse::<DeviceConfig>().is_err());
        assert!("warboy*".parse::<DeviceConfig>().is_err());
        assert!("*1".parse::<DeviceConfig>().is_err());
        assert!("some_npu*10".parse::<DeviceConfig>().is_err());
        assert!("warboy(2*10".parse::<DeviceConfig>().is_err());
        assert_eq!(
            "warboy(1)*2".parse::<DeviceConfig>(),
            Ok(DeviceConfig::Unnamed {
                arch: Arch::Warboy,
                mode: DeviceMode::Single,
                count: 2
            })
        );
        assert_eq!(
            "warboy(2)*4".parse::<DeviceConfig>(),
            Ok(DeviceConfig::Unnamed {
                arch: Arch::Warboy,
                mode: DeviceMode::Fusion,
                count: 4
            })
        );
        assert_eq!(
            "warboy*12".parse::<DeviceConfig>(),
            Ok(DeviceConfig::Unnamed {
                arch: Arch::Warboy,
                mode: DeviceMode::MultiCore,
                count: 12
            })
        );
        // assert!("npu*10".parse::<DeviceConfig>().is_ok());

        Ok(())
    }
}
