//! POC: 用 Windows Installer (MSI) API 枚举已装产品 + 取 InstallLocation/元数据，
//! 并演示按组件取产品的文件 keypath——验证"MSI 精确清单"这条高置信 footprint 来源。
//!
//! 无需管理员。验证点：
//!   1. MsiEnumProductsW 能否枚举全部已装 MSI 产品。
//!   2. MsiGetProductInfoW 能否取到 ProductName/Publisher/Version/InstallLocation。
//!   3. 有多少产品带 InstallLocation（决定我们对 InstallLocation 的依赖度）。
//!   4. MsiEnumComponentsW + MsiGetComponentPathW 能否取到某产品的真实文件路径 + 耗时。

use std::time::Instant;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::ApplicationInstallationAndServicing::{
    MsiEnumComponentsW, MsiEnumProductsW, MsiGetComponentPathW, MsiGetProductInfoW,
    INSTALLSTATE_LOCAL, INSTALLSTATE_SOURCE,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// 读某产品的一个属性（InstallLocation/ProductName/Publisher/VersionString...）。
fn product_info(product: &[u16], prop: &str) -> String {
    let propw = wide(prop);
    let mut buf = [0u16; 1024];
    let mut cch = buf.len() as u32;
    let r = unsafe {
        MsiGetProductInfoW(
            PCWSTR(product.as_ptr()),
            PCWSTR(propw.as_ptr()),
            PWSTR(buf.as_mut_ptr()),
            Some(&mut cch),
        )
    };
    if r == ERROR_SUCCESS.0 {
        from_wide(&buf[..(cch as usize).min(buf.len())])
    } else {
        String::new()
    }
}

fn main() {
    // 1) 枚举所有已装 MSI 产品（每个是 38 字符的 ProductCode GUID）
    let mut products: Vec<[u16; 39]> = Vec::new();
    let mut idx = 0u32;
    loop {
        let mut code = [0u16; 39];
        let r = unsafe { MsiEnumProductsW(idx, PWSTR(code.as_mut_ptr())) };
        if r == ERROR_NO_MORE_ITEMS.0 {
            break;
        }
        if r != ERROR_SUCCESS.0 {
            println!("MsiEnumProductsW 在 idx={} 返回错误码 {}", idx, r);
            break;
        }
        products.push(code);
        idx += 1;
    }

    println!("== MSI 已装产品：{} 个 ==", products.len());
    let with_loc = products
        .iter()
        .filter(|c| !product_info(&c[..], "InstallLocation").is_empty())
        .count();
    println!(
        "其中带 InstallLocation 的：{} / {}（其余需靠组件 keypath 或注册表定位）\n",
        with_loc,
        products.len()
    );

    for code in products.iter().take(15) {
        let name = product_info(&code[..], "ProductName");
        let pubr = product_info(&code[..], "Publisher");
        let ver = product_info(&code[..], "VersionString");
        let loc = product_info(&code[..], "InstallLocation");
        println!(
            "• {} | {} | v{}",
            if name.is_empty() { "(无名)".to_string() } else { name },
            if pubr.is_empty() { "(无发行商)".to_string() } else { pubr },
            ver
        );
        println!("    InstallLocation = {}", if loc.is_empty() { "(空)".to_string() } else { loc });
    }
    println!();

    // 2) 演示：对第一个"有名字且有安装位置"的产品，扫描组件取其文件 keypath
    let demo = products
        .iter()
        .find(|c| !product_info(&c[..], "ProductName").is_empty());
    if let Some(code) = demo {
        let name = product_info(&code[..], "ProductName");
        println!("== 演示组件 keypath：产品「{}」 ==", name);
        let product_pcwstr = PCWSTR(code.as_ptr());
        let started = Instant::now();
        let mut found: Vec<String> = Vec::new();
        let mut scanned = 0u64;
        let mut cidx = 0u32;
        loop {
            let mut comp = [0u16; 39];
            let r = unsafe { MsiEnumComponentsW(cidx, PWSTR(comp.as_mut_ptr())) };
            if r == ERROR_NO_MORE_ITEMS.0 || r != ERROR_SUCCESS.0 {
                break;
            }
            cidx += 1;
            scanned += 1;

            let mut pathbuf = [0u16; 1024];
            let mut pcch = pathbuf.len() as u32;
            let state = unsafe {
                MsiGetComponentPathW(
                    product_pcwstr,
                    PCWSTR(comp.as_ptr()),
                    PWSTR(pathbuf.as_mut_ptr()),
                    Some(&mut pcch),
                )
            };
            if state == INSTALLSTATE_LOCAL || state == INSTALLSTATE_SOURCE {
                let p = from_wide(&pathbuf[..(pcch as usize).min(pathbuf.len())]);
                if !p.is_empty() {
                    found.push(p);
                }
            }
            // 限制规模，避免 POC 跑太久（真实模块会缓存组件→产品映射）
            if scanned >= 40000 || found.len() >= 60 {
                break;
            }
        }
        println!(
            "扫描组件 {} 个，属于该产品的 keypath {} 条，用时 {} ms",
            scanned,
            found.len(),
            started.elapsed().as_millis()
        );
        for p in found.iter().take(30) {
            println!("    {}", p);
        }
    }
}
