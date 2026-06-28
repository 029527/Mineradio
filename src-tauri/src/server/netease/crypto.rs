//! 网易云加密信封 —— 1:1 复刻 NeteaseCloudMusicApi/util/crypto.js
//!
//! - weapi：两层 AES-128-CBC（presetKey + 随机 secretKey）+ 裸 RSA(encSecKey)
//! - eapi ：AES-128-ECB(hex)，明文 `nobody{url}use{data}md5forencrypt` 派生
//! - linuxapi：AES-128-ECB(hex)
//!
//! 全部 PKCS7 填充。CBC 用固定 IV `0102030405060708`，ECB 无 IV。
//! 输出格式与 crypto-js 完全一致：base64 / 大写 hex；MD5 小写 hex；RSA 小写 hex。

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit, KeyIvInit};
use aes::Aes128;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use md5::{Digest, Md5};
use num_bigint_dig::BigUint;
use rand::Rng;

type Aes128CbcEnc = cbc::Encryptor<Aes128>;
type Aes128EcbEnc = ecb::Encryptor<Aes128>;

const IV: &[u8] = b"0102030405060708";
const PRESET_KEY: &[u8] = b"0CoJUm6Qyw8W8jud";
const LINUXAPI_KEY: &[u8] = b"rFgB&h#%2?^eDg:Q";
const EAPI_KEY: &[u8] = b"e82ckenh8dichen8";
const BASE62: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

// RSA 公钥（与 crypto.js 中 PEM 等价），裸模幂 e=65537。
const RSA_N_HEX: &str = "e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7";
const RSA_MODLEN: usize = 128;

/// AES-128-CBC + PKCS7 → base64
fn aes_cbc_base64(text: &[u8], key: &[u8]) -> String {
    let enc = Aes128CbcEnc::new_from_slices(key, IV).expect("AES-CBC 密钥/IV 长度错误");
    STANDARD.encode(enc.encrypt_padded_vec_mut::<Pkcs7>(text))
}

/// AES-128-ECB + PKCS7 → 大写 hex
fn aes_ecb_hex_upper(text: &[u8], key: &[u8]) -> String {
    let enc = Aes128EcbEnc::new_from_slice(key).expect("AES-ECB 密钥长度错误");
    hex::encode_upper(enc.encrypt_padded_vec_mut::<Pkcs7>(text))
}

/// 裸 RSA（无填充）：data 右对齐零填充到模长 → m^e mod n → 模长大端 → 小写 hex
fn rsa_no_padding(data: &[u8]) -> String {
    let n = BigUint::parse_bytes(RSA_N_HEX.as_bytes(), 16).expect("RSA 模数解析失败");
    let e = BigUint::from(65537u32);

    let mut m = vec![0u8; RSA_MODLEN];
    let start = RSA_MODLEN - data.len();
    m[start..].copy_from_slice(data);

    let c = BigUint::from_bytes_be(&m).modpow(&e, &n);
    let c_bytes = c.to_bytes_be();

    let mut buf = vec![0u8; RSA_MODLEN];
    let cstart = RSA_MODLEN - c_bytes.len();
    buf[cstart..].copy_from_slice(&c_bytes);
    hex::encode(buf)
}

/// 生成 16 字节 base62 随机 secretKey（生产用）
pub fn random_secret_key() -> [u8; 16] {
    let mut rng = rand::thread_rng();
    let mut key = [0u8; 16];
    for b in key.iter_mut() {
        *b = BASE62[rng.gen_range(0..BASE62.len())];
    }
    key
}

/// weapi 信封。`json_text` 必须是已序列化好的 JSON 字符串。
pub fn weapi_with_key(json_text: &str, secret_key: &[u8; 16]) -> (String, String) {
    let inner = aes_cbc_base64(json_text.as_bytes(), PRESET_KEY);
    let params = aes_cbc_base64(inner.as_bytes(), secret_key);
    let reversed: Vec<u8> = secret_key.iter().rev().copied().collect();
    let enc_sec_key = rsa_no_padding(&reversed);
    (params, enc_sec_key)
}

/// weapi 信封（随机 secretKey，生产用）。返回 (params, encSecKey)。
pub fn weapi(json_text: &str) -> (String, String) {
    weapi_with_key(json_text, &random_secret_key())
}

/// eapi 信封。`url` 为原始 `/api/...` 路径，`json_text` 为已序列化 JSON。
pub fn eapi(url: &str, json_text: &str) -> String {
    let message = format!("nobody{url}use{json_text}md5forencrypt");
    let digest = hex::encode(Md5::digest(message.as_bytes()));
    let data = format!("{url}-36cd479b6b5-{json_text}-36cd479b6b5-{digest}");
    aes_ecb_hex_upper(data.as_bytes(), EAPI_KEY)
}

/// linuxapi 信封。返回 eparams（大写 hex）。
pub fn linuxapi(json_text: &str) -> String {
    aes_ecb_hex_upper(json_text.as_bytes(), LINUXAPI_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 以下期望值由 Node 端 NeteaseCloudMusicApi/util/crypto.js 在固定输入下生成，
    // 见 scratchpad/ref.js。Math.random 固定使 weapi secretKey = "FFFFFFFFFFFFFFFF"。

    const SEARCH_JSON: &str = r#"{"s":"周杰伦","type":1,"limit":3,"offset":0}"#;

    #[test]
    fn aes_cbc_inner_matches() {
        assert_eq!(
            aes_cbc_base64(SEARCH_JSON.as_bytes(), PRESET_KEY),
            "V9Vu+x+fuurEfbP2qyBZjLW8Y/9MQpQAQ/OgvaAIsG/tmAWgssfVpNq1hnxrsLBV"
        );
    }

    #[test]
    fn aes_ecb_hex_matches() {
        assert_eq!(
            aes_ecb_hex_upper(b"hello-mineradio", EAPI_KEY),
            "9C3D996B5AC758FE1883B6FE7445ADFA"
        );
    }

    #[test]
    fn eapi_song_url_matches() {
        let json = r#"{"ids":"[33894312]","level":"standard","encodeType":"flac"}"#;
        assert_eq!(
            eapi("/api/song/enhance/player/url/v1", json),
            "FA90B329E9614F79E79598F37DC2EDB487F00D1BC4C9B24CD57E6C318B9073569338432CD7D98D1A3626E997A2C531219F7DD8FF31B2E259DBCA22F31A11628D729C1320A7E748C7FDBC2F81AA28A1A0009D4EE3365E9D57819BE2BB7A5D92950E8E9F65B9FBA177C7E46C7641D675200C0A40D6F219EE69A356AF0158AF39254EABFB6A185A260E32B0883BCF3982304A6165EC7AD759D2A21D27A1569248E4"
        );
    }

    #[test]
    fn eapi_search_matches() {
        assert_eq!(
            eapi("/api/search/get", SEARCH_JSON),
            "1AF0E93B0E3EA03CE4E7F1B6AD7BD32BC198D7B70109AB343E0FC0C4A8F27C961571CE0B0B83772535D0B42745D985CD2AEE30ECF32962C040C9BEB6A7841E9922B5AF3DE362FA00A5B6F45A5D7EE470E8AEF15F540BF0F40EA619B62D92B5F3026E4DF84133E8B956F2AE702B3435E81F0146E084F89C0B690DDF88556A511B"
        );
    }

    #[test]
    fn weapi_matches_fixed_key() {
        let key = b"FFFFFFFFFFFFFFFF";
        let (params, enc_sec_key) = weapi_with_key(SEARCH_JSON, key);
        assert_eq!(
            params,
            "5M/EpDowAIQUUXwhdj8oP6NU4iFx3mZIpbbIVf7UGuJdE1/VoyISAxasf+RGTKF837IRhDf91V4UoQOX6uznGSvzekv9e5EvXulPFdka+/c="
        );
        assert_eq!(
            enc_sec_key,
            "257348aecb5e556c066de214e531faadd1c55d814f9be95fd06d6bff9f4c7a41f831f6394d5a3fd2e3881736d94a02ca919d952872e7d0a50ebfa1769a7a62d512f5f1ca21aec60bc3819a9c3ffca5eca9a0dba6d6f7249b06f5965ecfff3695b54e1c28f3f624750ed39e7de08fc8493242e26dbc4484a01c76f739e135637c"
        );
    }

    #[test]
    fn linuxapi_matches() {
        let json = r#"{"method":"POST","url":"https://music.163.com/api/search/get","params":{"s":"x"}}"#;
        assert_eq!(
            linuxapi(json),
            "A0D9583F4C5FF68DE851D2893A49DE98FAFB24399F27B4F7E74C64B6FC49A965CFA972FA5EA3D6247CD6247C8198CB8770AAB1AA0DA78F0EB88EF1E1C88A47242F9C7ADE9DBB04F7FF59ECD6653C3247FC5965DED0B85514DD5B3281C0C36ECB"
        );
    }
}
