//! Geo-IP 查询：通过 ip-api.com 免费 API 查询 IP 地理位置。
//! 内置简易 LRU 缓存（128 条），避免重复请求。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct GeoInfo {
    pub ip: String,
    pub country: String,
    pub city: String,
    pub isp: String,
}

static CACHE: LazyLock<Mutex<HashMap<String, Option<GeoInfo>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn lookup(ip: &str) -> Option<GeoInfo> {
    // Check cache first
    if let Ok(cache) = CACHE.lock() {
        if let Some(cached) = cache.get(ip) {
            return cached.clone();
        }
    }

    // Query ip-api.com (free, no key, 45 req/min limit)
    let url = format!(
        "http://ip-api.com/json/{}?fields=country,city,isp,query",
        ip
    );
    let resp = reqwest::blocking::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .ok()?;

    #[derive(serde::Deserialize)]
    struct ApiResp {
        query: Option<String>,
        country: Option<String>,
        city: Option<String>,
        isp: Option<String>,
    }

    let ar: ApiResp = resp.json().ok()?;
    let info = GeoInfo {
        ip: ar.query.unwrap_or_else(|| ip.to_string()),
        country: ar.country.unwrap_or_else(|| "-".to_string()),
        city: ar.city.unwrap_or_default(),
        isp: ar.isp.unwrap_or_default(),
    };

    // Update cache, cap at 128 entries
    if let Ok(mut cache) = CACHE.lock() {
        if cache.len() > 128 {
            cache.clear();
        }
        cache.insert(ip.to_string(), Some(info.clone()));
    }

    Some(info)
}
