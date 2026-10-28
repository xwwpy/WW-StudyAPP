//! WebDAV secrets are protected for the current Windows account with DPAPI.

use anyhow::{Result, ensure};
use zeroize::Zeroizing;

const CONTEXT: &[u8] = b"StudyApp/WebDAV/password/v1\0";
const IMAGE_CONTEXT: &[u8] = b"StudyApp/ImageCloud/secret/v1\0";

pub fn protect_image(secret: &str) -> Result<Vec<u8>> {
    ensure!(secret.len() <= 16_384, "图片密钥长度超过限制");
    let mut plaintext = Zeroizing::new(IMAGE_CONTEXT.to_vec());
    plaintext.extend_from_slice(secret.as_bytes());
    crypt(&plaintext, true)
}

pub fn unprotect_image(ciphertext: &[u8]) -> Result<Zeroizing<String>> {
    ensure!(ciphertext.len() <= 65_536, "图片凭据文件超过限制");
    let plaintext = Zeroizing::new(crypt(ciphertext, false)?);
    ensure!(
        plaintext.starts_with(IMAGE_CONTEXT),
        "图片凭据格式无效，请重新填写 Secret"
    );
    Ok(Zeroizing::new(
        String::from_utf8(plaintext[IMAGE_CONTEXT.len()..].to_vec())
            .map_err(|_| anyhow::anyhow!("图片凭据内容无效，请重新填写 Secret"))?,
    ))
}

pub fn protect(password: &str) -> Result<Vec<u8>> {
    ensure!(password.len() <= 16_384, "密码长度超过限制");
    let mut plaintext = Zeroizing::new(CONTEXT.to_vec());
    plaintext.extend_from_slice(password.as_bytes());
    crypt(&plaintext, true)
}

pub fn unprotect(ciphertext: &[u8]) -> Result<Zeroizing<String>> {
    ensure!(ciphertext.len() <= 65_536, "凭据文件超过限制");
    let plaintext = Zeroizing::new(crypt(ciphertext, false)?);
    ensure!(
        plaintext.starts_with(CONTEXT),
        "凭据格式无效，请重新填写密码"
    );
    let password = String::from_utf8(plaintext[CONTEXT.len()..].to_vec())
        .map_err(|_| anyhow::anyhow!("凭据内容无效，请重新填写密码"))?;
    Ok(Zeroizing::new(password))
}

#[cfg(windows)]
fn crypt(input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    use zeroize::Zeroize;
    let source = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len())?,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut destination = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // DPAPI treats source as input and allocates destination with LocalAlloc.
    // The default scope binds it to this Windows user, never LOCAL_MACHINE.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &source,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut destination,
            )
        } else {
            CryptUnprotectData(
                &source,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut destination,
            )
        }
    };
    ensure!(
        success != 0,
        "无法访问此 Windows 用户的受保护凭据，请重新填写密码"
    );
    ensure!(!destination.pbData.is_null(), "Windows 凭据保护返回空数据");
    // Copy before freeing the OS allocation; wipe decrypted bytes in that allocation.
    let output = unsafe {
        let bytes = std::slice::from_raw_parts_mut(destination.pbData, destination.cbData as usize);
        let output = bytes.to_vec();
        bytes.zeroize();
        LocalFree(destination.pbData.cast());
        output
    };
    Ok(output)
}

#[cfg(not(windows))]
fn crypt(_: &[u8], _: bool) -> Result<Vec<u8>> {
    anyhow::bail!("当前版本仅在 Windows 上提供系统保护的凭据存储")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn credentials_roundtrip_without_plaintext_storage() {
        let password = "仅供测试-application-password";
        let protected = protect(password).unwrap();
        assert!(
            !protected
                .windows(password.len())
                .any(|part| part == password.as_bytes())
        );
        assert_eq!(unprotect(&protected).unwrap().as_str(), password);
    }

    #[test]
    fn damaged_or_foreign_credentials_are_rejected() {
        assert!(unprotect(b"not-a-dpapi-blob").is_err());
        let mut protected = protect("fixture").unwrap();
        let middle = protected.len() / 2;
        protected[middle] ^= 0xff;
        assert!(unprotect(&protected).is_err());
    }

    #[test]
    fn unreasonable_credentials_are_rejected() {
        assert!(protect(&"x".repeat(16_385)).is_err());
        assert!(unprotect(&vec![0; 65_537]).is_err());
    }
}
