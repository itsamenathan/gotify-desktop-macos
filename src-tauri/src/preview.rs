use std::{
    net::{IpAddr, Ipv6Addr, ToSocketAddrs},
    time::Duration,
};

use crate::{
    UrlPreview, PREVIEW_MAX_HTML_BYTES, PREVIEW_MAX_REDIRECTS, PREVIEW_REQUEST_TIMEOUT_SECS,
};

pub(crate) async fn fetch_url_preview(url: String) -> Result<UrlPreview, String> {
    let mut current_url =
        reqwest::Url::parse(url.trim()).map_err(|error| format!("Invalid preview URL: {error}"))?;
    enforce_preview_target_policy(&current_url).await?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PREVIEW_REQUEST_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("Failed to build preview HTTP client: {error}"))?;

    for redirect_hops in 0..=PREVIEW_MAX_REDIRECTS {
        let response = client
            .get(current_url.clone())
            .header(
                reqwest::header::USER_AGENT,
                "Gotify-Desktop-Preview/1.0 (+https://gotify.net)",
            )
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .send()
            .await
            .map_err(|error| format!("Preview request failed: {error}"))?;

        if response.status().is_redirection() {
            if redirect_hops == PREVIEW_MAX_REDIRECTS {
                return Err(format!(
                    "Preview request redirected too many times (>{PREVIEW_MAX_REDIRECTS})"
                ));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| "Preview redirect missing location header".to_string())?;
            let location_value = location
                .to_str()
                .map_err(|error| format!("Preview redirect location is invalid: {error}"))?;
            current_url = resolve_preview_redirect_url(&current_url, location_value)?;
            enforce_preview_target_policy(&current_url).await?;
            continue;
        }

        if !response.status().is_success() {
            return Err(format!(
                "Preview request failed with HTTP {}",
                response.status().as_u16()
            ));
        }

        if let Some(content_length) = response.content_length() {
            if content_length > PREVIEW_MAX_HTML_BYTES as u64 {
                return Err(format!(
                    "Preview response too large ({content_length} bytes > {PREVIEW_MAX_HTML_BYTES} bytes)"
                ));
            }
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !content_type.contains("text/html") {
            return Ok(UrlPreview {
                url: current_url.to_string(),
                title: None,
                description: None,
                site_name: current_url.host_str().map(ToString::to_string),
                image: None,
            });
        }

        let body = read_limited_preview_body(response, PREVIEW_MAX_HTML_BYTES).await?;
        let title = find_meta(&body, &["og:title"]).or_else(|| find_title(&body));
        let description = find_meta(&body, &["og:description", "description"]);
        let site_name = find_meta(&body, &["og:site_name"])
            .or_else(|| current_url.host_str().map(ToString::to_string));
        let image = find_meta(&body, &["og:image"])
            .and_then(|value| resolve_meta_url(&current_url, &value));

        return Ok(UrlPreview {
            url: current_url.to_string(),
            title,
            description,
            site_name,
            image,
        });
    }

    Err("Preview request failed after redirects".to_string())
}

fn ensure_preview_http_scheme(url: &reqwest::Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(format!(
            "Only http/https URLs are supported for previews (got '{other}')"
        )),
    }
}

async fn enforce_preview_target_policy(url: &reqwest::Url) -> Result<(), String> {
    ensure_preview_http_scheme(url)?;

    let host = url
        .host_str()
        .ok_or_else(|| "Preview URL is missing a host".to_string())?;
    if is_blocked_preview_hostname(host) {
        return Err(format!("Preview blocked for restricted hostname '{host}'"));
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if let Some(reason) = preview_block_reason_for_ip(ip) {
            return Err(format!("Preview blocked for {reason} target '{ip}'"));
        }
    } else {
        let port = url
            .port_or_known_default()
            .ok_or_else(|| "Preview URL missing a known port for scheme".to_string())?;
        let ips = resolve_preview_domain_ips(host, port).await?;
        for ip in ips {
            if let Some(reason) = preview_block_reason_for_ip(ip) {
                return Err(format!(
                    "Preview blocked for {reason} target (domain '{host}' resolved to {ip})"
                ));
            }
        }
    }

    Ok(())
}

fn is_blocked_preview_hostname(host: &str) -> bool {
    let normalized = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    if normalized == "localhost" || normalized.ends_with(".localhost") {
        return true;
    }
    matches!(
        normalized.as_str(),
        "metadata"
            | "metadata.google.internal"
            | "metadata.azure.internal"
            | "instance-data.ec2.internal"
    )
}

fn preview_block_reason_for_ip(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_unspecified() {
                return Some("unspecified");
            }
            if v4.is_loopback() {
                return Some("loopback");
            }
            if v4.is_link_local() {
                return Some("link-local");
            }
            let octets = v4.octets();
            if matches!(
                octets,
                [169, 254, 169, 254] | [169, 254, 170, 2] | [100, 100, 100, 200]
            ) {
                return Some("metadata endpoint");
            }
            None
        }
        IpAddr::V6(v6) => {
            if v6.is_unspecified() {
                return Some("unspecified");
            }
            if v6.is_loopback() {
                return Some("loopback");
            }
            if v6.is_unicast_link_local() {
                return Some("link-local");
            }
            if v6 == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254) {
                return Some("metadata endpoint");
            }
            None
        }
    }
}

async fn resolve_preview_domain_ips(domain: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    let domain_for_lookup = domain.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let mut ips = Vec::new();
        let addrs = (domain_for_lookup.as_str(), port)
            .to_socket_addrs()
            .map_err(|error| {
                format!("Failed to resolve preview host '{domain_for_lookup}': {error}")
            })?;
        for addr in addrs {
            let ip = addr.ip();
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        }
        if ips.is_empty() {
            return Err(format!(
                "Failed to resolve preview host '{domain_for_lookup}' to an IP address"
            ));
        }
        Ok(ips)
    })
    .await
    .map_err(|error| format!("Failed to join DNS lookup task: {error}"))?
}

fn resolve_preview_redirect_url(
    current_url: &reqwest::Url,
    location: &str,
) -> Result<reqwest::Url, String> {
    let trimmed = location.trim();
    if trimmed.is_empty() {
        return Err("Preview redirect location is empty".to_string());
    }
    let next = current_url
        .join(trimmed)
        .map_err(|error| format!("Invalid preview redirect location: {error}"))?;
    ensure_preview_http_scheme(&next)?;
    Ok(next)
}

async fn read_limited_preview_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<String, String> {
    let mut out = Vec::new();
    loop {
        let next_chunk = response
            .chunk()
            .await
            .map_err(|error| format!("Failed to read preview response body: {error}"))?;
        let Some(chunk) = next_chunk else {
            break;
        };
        if out.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("Preview response exceeded {max_bytes} byte limit"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

fn find_title(html: &str) -> Option<String> {
    let doc = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("title").ok()?;
    let el = doc.select(&selector).next()?;
    let text: String = el.text().collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn find_meta(html: &str, keys: &[&str]) -> Option<String> {
    let doc = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("meta").ok()?;
    for el in doc.select(&selector) {
        let prop = el
            .value()
            .attr("property")
            .or_else(|| el.value().attr("name"))
            .unwrap_or("");
        let prop_lower = prop.to_ascii_lowercase();
        if keys.iter().any(|k| prop_lower == k.to_ascii_lowercase()) {
            if let Some(content) = el.value().attr("content") {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn resolve_meta_url(base_url: &reqwest::Url, raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    let resolved = if let Ok(url) = reqwest::Url::parse(raw) {
        url
    } else {
        base_url.join(raw).ok()?
    };
    if !matches!(resolved.scheme(), "http" | "https") {
        return None;
    }
    if let Some(host) = resolved.host_str() {
        if is_blocked_preview_hostname(host) {
            return None;
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if preview_block_reason_for_ip(ip).is_some() {
                return None;
            }
        }
    }
    Some(resolved.to_string())
}
