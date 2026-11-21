use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const ENTROPY: &[u8] = b"AuraQuota-DPAPI-v1";

#[cfg(windows)]
pub fn protect_string(plaintext: &str) -> Result<String, String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let bytes = plaintext.as_bytes();
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let entropy_blob = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };

    let ok = unsafe {
        CryptProtectData(
            &in_blob,
            null_mut(),
            &entropy_blob,
            null_mut(),
            null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };

    if ok == 0 || out_blob.pbData.is_null() {
        return Err("Windows DPAPI CryptProtectData 加密失败".to_string());
    }

    let encrypted_slice =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
    let encoded = STANDARD.encode(encrypted_slice);
    unsafe {
        LocalFree(out_blob.pbData as _);
    }

    Ok(encoded)
}

#[cfg(windows)]
pub fn unprotect_string(b64_cipher: &str) -> Result<String, String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let cipher_bytes = STANDARD
        .decode(b64_cipher.trim())
        .map_err(|e| format!("DPAPI Base64 解码失败: {e}"))?;

    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: cipher_bytes.len() as u32,
        pbData: cipher_bytes.as_ptr() as *mut u8,
    };
    let entropy_blob = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };

    let ok = unsafe {
        CryptUnprotectData(
            &in_blob,
            null_mut(),
            &entropy_blob,
            null_mut(),
            null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };

    if ok == 0 || out_blob.pbData.is_null() {
        return Err("Windows DPAPI CryptUnprotectData 解密失败".to_string());
    }

    let plain_slice =
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
    let result = String::from_utf8(plain_slice.to_vec())
        .map_err(|e| format!("DPAPI UTF-8 转换失败: {e}"));
    unsafe {
        LocalFree(out_blob.pbData as _);
    }

    result
}

#[cfg(not(windows))]
pub fn protect_string(_plaintext: &str) -> Result<String, String> {
    Err("DPAPI encryption is only supported on Windows".to_string())
}

#[cfg(not(windows))]
pub fn unprotect_string(_b64_cipher: &str) -> Result<String, String> {
    Err("DPAPI decryption is only supported on Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let sample_payload = "mock_dpapi_roundtrip_sample_value";
        let encrypted =
            protect_string(sample_payload).expect("DPAPI protect should succeed on Windows");
        assert_ne!(encrypted, sample_payload);
        assert!(!encrypted.contains(sample_payload));
        let decrypted = unprotect_string(&encrypted).expect("DPAPI unprotect should succeed");
        assert_eq!(decrypted, sample_payload);
    }

    #[test]
    fn dpapi_ciphertext_hides_plaintext() {
        let sample_payload = "mock_dpapi_hidden_payload_9988776655";
        let encrypted = protect_string(sample_payload).expect("protect");
        let raw_bytes = STANDARD.decode(&encrypted).expect("valid base64");
        let lossy = String::from_utf8_lossy(&raw_bytes);
        assert!(!lossy.contains("mock_dpapi_hidden_payload"));
    }
}
