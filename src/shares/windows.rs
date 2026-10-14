use super::{Backend, MappingError, ResourceType, Share};
use windows_sys::Win32::{Foundation::*, NetworkManagement::WNet::*};

pub struct Network;

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn connection(target: &str) -> Result<Option<String>, u32> {
    let target = wide(target);
    let mut buffer = vec![0u16; 512];
    loop {
        let mut length = buffer.len() as u32;
        let code = unsafe { WNetGetConnectionW(target.as_ptr(), buffer.as_mut_ptr(), &mut length) };
        match code {
            NO_ERROR => {
                let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
                return Ok(Some(String::from_utf16_lossy(&buffer[..end])));
            }
            ERROR_NOT_CONNECTED => return Ok(None),
            ERROR_MORE_DATA if length > buffer.len() as u32 && length <= 32768 => {
                buffer.resize(length as usize, 0)
            }
            _ => return Err(code),
        }
    }
}

fn mapping_error(code: u32) -> MappingError {
    match code {
        ERROR_ALREADY_ASSIGNED | ERROR_DEVICE_ALREADY_REMEMBERED | ERROR_CONNECTION_UNAVAIL => {
            MappingError::Premapped
        }
        ERROR_BAD_NETPATH
        | ERROR_BAD_NET_NAME
        | ERROR_NO_NETWORK
        | ERROR_NO_NET_OR_BAD_PATH
        | ERROR_NETWORK_BUSY
        | ERROR_BUSY
        | ERROR_RETRY
        | ERROR_SEM_TIMEOUT
        | ERROR_NETWORK_UNREACHABLE
        | ERROR_HOST_UNREACHABLE
        | ERROR_CONNECTION_REFUSED => MappingError::Temporary(code),
        _ => MappingError::Permanent(code),
    }
}

impl Backend for Network {
    fn map(&mut self, share: &Share) -> Result<(), MappingError> {
        if connection(&share.target).map_err(mapping_error)?.is_some() {
            return Err(MappingError::Premapped);
        }
        let mut target = wide(&share.target);
        let mut location = wide(&share.location);
        let account = share.account.as_deref().map(wide);
        // Match JSW: password has no effect without an explicit account.
        let mut password = share
            .account
            .as_ref()
            .and(share.password.as_deref())
            .map(wide);
        let resource = NETRESOURCEW {
            dwType: match share.resource_type {
                // WNet requires a concrete resource type when redirecting a device.
                ResourceType::Any if share.target.len() == 2 => RESOURCETYPE_DISK,
                ResourceType::Any => RESOURCETYPE_PRINT,
                ResourceType::Disk => RESOURCETYPE_DISK,
                ResourceType::Printer => RESOURCETYPE_PRINT,
            },
            lpLocalName: target.as_mut_ptr(),
            lpRemoteName: location.as_mut_ptr(),
            ..Default::default()
        };
        // No prompt and no persistent user-profile entry. Credentials never enter a command line.
        let code = unsafe {
            WNetAddConnection2W(
                &resource,
                password.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                account.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                0,
            )
        };
        if let Some(password) = &mut password {
            password.fill(0);
        }
        if code == NO_ERROR {
            Ok(())
        } else {
            Err(mapping_error(code))
        }
    }

    fn unmap(&mut self, share: &Share) -> Result<(), u32> {
        // Another process may have removed or repointed the drive since startup.
        let Some(remote) = connection(&share.target)? else {
            return Ok(());
        };
        if !remote.eq_ignore_ascii_case(&share.location) {
            return Ok(());
        }
        let target = wide(&share.target);
        let code = unsafe { WNetCancelConnection2W(target.as_ptr(), 0, i32::from(share.force)) };
        if matches!(code, NO_ERROR | ERROR_NOT_CONNECTED) {
            Ok(())
        } else {
            Err(code)
        }
    }
}
