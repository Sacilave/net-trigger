//! 锐捷 RG-SAM+ / ePortal 密码安全与加密模块
//!
//! 遵循锐捷 security.js / login_bch.js 官方规范：
//! 1. 待加密明文: `password + ">" + mac`
//! 2. 字符串逆序反转: `.chars().rev().collect()`
//! 3. 16 位小端字打包为 BigUint: chunkSize = 2 * (digits - 1)
//! 4. 模幂运算: `block.modpow(e, m)`
//! 5. 16 位大端十六进制输出格式化

use std::time::Duration;

/// 锐捷 SAM+ 标准 RSA 算法加密密码
pub fn rsa_encrypt_ruijie(plain: &str, exponent_hex: &str, modulus_hex: &str) -> Option<String> {
    use num_bigint::BigUint;
    use num_traits::Num;

    let e = BigUint::from_str_radix(exponent_hex, 16).ok()?;
    let m = BigUint::from_str_radix(modulus_hex, 16).ok()?;

    let m_bytes = m.to_bytes_be();
    let num_digits_16 = (m_bytes.len() + 1) / 2;
    let high_index = num_digits_16.saturating_sub(1);
    let chunk_size = 2 * high_index;
    if chunk_size == 0 {
        return None;
    }

    let mut a: Vec<u8> = plain.bytes().collect();
    while a.len() % chunk_size != 0 {
        a.push(0);
    }

    let mut result = String::new();

    for chunk in a.chunks(chunk_size) {
        let mut words_16 = Vec::with_capacity(num_digits_16);
        for pair in chunk.chunks(2) {
            let lo = pair[0] as u16;
            let hi = if pair.len() > 1 { (pair[1] as u16) << 8 } else { 0 };
            words_16.push(lo | hi);
        }
        while words_16.len() < num_digits_16 {
            words_16.push(0);
        }

        let mut bytes_le = Vec::with_capacity(num_digits_16 * 2);
        for w in &words_16 {
            bytes_le.push((w & 0xff) as u8);
            bytes_le.push((w >> 8) as u8);
        }
        let block_val = BigUint::from_bytes_le(&bytes_le);

        let encrypted = block_val.modpow(&e, &m);

        let enc_bytes_le = encrypted.to_bytes_le();
        let mut enc_words = Vec::with_capacity(num_digits_16);
        for pair in enc_bytes_le.chunks(2) {
            let lo = pair[0] as u16;
            let hi = if pair.len() > 1 { (pair[1] as u16) << 8 } else { 0 };
            enc_words.push(lo | hi);
        }
        while enc_words.len() < num_digits_16 {
            enc_words.push(0);
        }

        let hi = enc_words.len().saturating_sub(1);
        let mut hex_chunk = String::new();
        for i in (0..=hi).rev() {
            use std::fmt::Write;
            let _ = write!(&mut hex_chunk, "{:04x}", enc_words[i]);
        }

        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(&hex_chunk);
    }

    Some(result)
}

/// 查询锐捷 SAM+ 网关页面配置（获取是否开启 RSA 密码加密及公钥指数和模数）
pub fn fetch_ruijie_page_info(host: &str, port: u16, query_string: &str) -> Option<(String, String)> {
    let agent = ureq::builder()
        .redirects(0)
        .timeout_connect(Duration::from_millis(2000))
        .timeout_read(Duration::from_millis(2500))
        .build();

    let page_info_url = format!("http://{}:{}/eportal/InterFace.do?method=pageInfo", host, port);
    let index_url = format!("http://{}:{}/eportal/index.jsp?{}", host, port, query_string);

    let body = format!("queryString={}", crate::auth::http_client::urlencoding_encode(query_string));

    let resp = agent
        .post(&page_info_url)
        .set("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
        .set("Referer", &index_url)
        .set("Origin", &format!("http://{}:{}", host, port))
        .set("X-Requested-With", "XMLHttpRequest")
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .send_string(&body)
        .ok()?;

    let mut buf = [0u8; 4096];
    let mut reader = resp.into_reader().take(4096);
    use std::io::Read;
    let n = reader.read(&mut buf).unwrap_or(0);
    let resp_str = String::from_utf8_lossy(&buf[..n]);

    if resp_str.contains("\"passwordEncrypt\":\"true\"") || resp_str.contains("\"passwordEncrypt\": \"true\"") {
        let exp = crate::auth::ruijie::parser::extract_json_field(&resp_str, "publicKeyExponent")
            .unwrap_or_else(|| "10001".to_string());
        let modulus = crate::auth::ruijie::parser::extract_json_field(&resp_str, "publicKeyModulus")?;
        return Some((exp, modulus));
    }

    None
}
