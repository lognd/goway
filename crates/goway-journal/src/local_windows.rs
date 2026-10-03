//! Windows registry access for `LocalSystem`: user environment variables and typed values.

use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_EXPAND_SZ,
    REG_SZ, RegType,
};
use winreg::{HKEY, RegKey, RegValue as RawValue};

use crate::change::RegValue;
use crate::error::SystemError;
use crate::system::SysResult;

/// Registry key holding the per-user environment variables.
const USER_ENV: &str = "Environment";

fn split_key(key: &str) -> SysResult<(HKEY, &str)> {
    let (root, rest) = key.split_once('\\').unwrap_or((key, ""));
    match root {
        "HKCU" | "HKEY_CURRENT_USER" => Ok((HKEY_CURRENT_USER, rest)),
        "HKLM" | "HKEY_LOCAL_MACHINE" => Ok((HKEY_LOCAL_MACHINE, rest)),
        _ => Err(SystemError::InvalidState(format!(
            "unknown registry root in {key}"
        ))),
    }
}

fn io(key: &str, source: std::io::Error) -> SystemError {
    SystemError::Io {
        path: key.into(),
        source,
    }
}

/// Open an existing key; `None` when it does not exist.
fn open(key: &str, flags: u32) -> SysResult<Option<RegKey>> {
    let (root, rest) = split_key(key)?;
    match RegKey::predef(root).open_subkey_with_flags(rest, flags) {
        Ok(k) => Ok(Some(k)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(key, e)),
    }
}

fn open_or_create(key: &str, flags: u32) -> SysResult<RegKey> {
    let (root, rest) = split_key(key)?;
    RegKey::predef(root)
        .create_subkey_with_flags(rest, flags)
        .map(|(k, _)| k)
        .map_err(|e| io(key, e))
}

fn decode_utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn encode_utf16(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn to_value(key: &str, name: &str, raw: &RawValue) -> SysResult<RegValue> {
    match &raw.vtype {
        &REG_SZ => Ok(RegValue::String(decode_utf16(&raw.bytes))),
        &REG_EXPAND_SZ => Ok(RegValue::ExpandString(decode_utf16(&raw.bytes))),
        &REG_DWORD if raw.bytes.len() == 4 => Ok(RegValue::Dword(u32::from_le_bytes([
            raw.bytes[0],
            raw.bytes[1],
            raw.bytes[2],
            raw.bytes[3],
        ]))),
        other => Err(SystemError::InvalidState(format!(
            "{key}\\{name} has unsupported registry type {other:?}"
        ))),
    }
}

fn from_value(value: &RegValue) -> RawValue {
    let (vtype, bytes): (RegType, Vec<u8>) = match value {
        RegValue::String(s) => (REG_SZ, encode_utf16(s)),
        RegValue::ExpandString(s) => (REG_EXPAND_SZ, encode_utf16(s)),
        RegValue::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
    };
    RawValue { bytes, vtype }
}

pub(crate) fn reg_get(key: &str, name: &str) -> SysResult<Option<RegValue>> {
    let Some(k) = open(key, KEY_READ)? else {
        return Ok(None);
    };
    match k.get_raw_value(name) {
        Ok(raw) => to_value(key, name, &raw).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(key, e)),
    }
}

pub(crate) fn reg_set(key: &str, name: &str, value: &RegValue) -> SysResult<()> {
    tracing::debug!(key, name, ?value, "windows: registry set");
    let k = open_or_create(key, KEY_SET_VALUE)?;
    k.set_raw_value(name, &from_value(value))
        .map_err(|e| io(key, e))
}

pub(crate) fn reg_delete(key: &str, name: &str) -> SysResult<()> {
    tracing::debug!(key, name, "windows: registry delete value");
    let Some(k) = open(key, KEY_SET_VALUE)? else {
        return Ok(());
    };
    match k.delete_value(name) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(key, e)),
        _ => Ok(()),
    }
}

pub(crate) fn reg_key_exists(key: &str) -> SysResult<bool> {
    Ok(open(key, KEY_READ)?.is_some())
}

pub(crate) fn reg_key_create(key: &str) -> SysResult<()> {
    tracing::debug!(key, "windows: registry create key");
    open_or_create(key, KEY_READ).map(drop)
}

pub(crate) fn reg_key_is_empty(key: &str) -> SysResult<bool> {
    let k = open(key, KEY_READ)?.ok_or_else(|| SystemError::NotFound(key.to_owned()))?;
    let info = k.query_info().map_err(|e| io(key, e))?;
    Ok(info.sub_keys == 0 && info.values == 0)
}

pub(crate) fn reg_key_remove(key: &str) -> SysResult<()> {
    tracing::debug!(key, "windows: registry remove key");
    if !reg_key_exists(key)? {
        return Ok(());
    }
    if !reg_key_is_empty(key)? {
        return Err(SystemError::InvalidState(format!("{key} is not empty")));
    }
    let (root, rest) = split_key(key)?;
    let (parent, leaf) = rest.rsplit_once('\\').unwrap_or(("", rest));
    let parent_key = RegKey::predef(root)
        .open_subkey_with_flags(parent, KEY_SET_VALUE)
        .map_err(|e| io(key, e))?;
    parent_key.delete_subkey(leaf).map_err(|e| io(key, e))
}

fn env_key() -> String {
    format!("HKCU\\{USER_ENV}")
}

/// A user environment variable's raw string, whatever its string type.
pub(crate) fn get_var(name: &str) -> SysResult<Option<String>> {
    Ok(match reg_get(&env_key(), name)? {
        Some(RegValue::String(s) | RegValue::ExpandString(s)) => Some(s),
        Some(RegValue::Dword(_)) => {
            return Err(SystemError::InvalidState(format!(
                "environment variable {name} is not a string"
            )));
        }
        None => None,
    })
}

/// Set a user environment variable, keeping an existing value's string type (new ones expand).
pub(crate) fn set_var(name: &str, value: &str) -> SysResult<()> {
    let key = env_key();
    let typed = match reg_get(&key, name)? {
        Some(RegValue::String(_)) => RegValue::String(value.to_owned()),
        _ => RegValue::ExpandString(value.to_owned()),
    };
    reg_set(&key, name, &typed)
}

pub(crate) fn remove_var(name: &str) -> SysResult<()> {
    reg_delete(&env_key(), name)
}
