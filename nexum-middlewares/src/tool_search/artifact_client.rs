const DEFAULT_URL: &str = "https://cloud-artifacts.claude-code-best.win";
const DEFAULT_TOKEN: &str = "claude-code-best";

/// CCB Artifacts 服务 HTTP 客户端。
pub struct ArtifactClient {
    base_url: String,
    token: String,
}

/// 服务端响应（兼容 Deno Deploy 抹平 HTTP 状态码的情况，错误字段在 body 中）
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ArtifactResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, alias = "expiresAt")]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ArtifactClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self { base_url, token }
    }

    /// 使用默认 CCB 服务端和内置 token。
    /// NEXUM_ARTIFACTS_URL/TOKEN 可覆盖；PERI_ARTIFACTS_* 为 legacy alias
    /// (ADR-NEXUM-IDENTITY-MIGRATION D4: prioridad NEXUM_* > PERI_*,
    /// alias con ventana de deprecación hasta el paso 2 del rename).
    pub fn from_env_or_default() -> Self {
        let url = std::env::var("NEXUM_ARTIFACTS_URL")
            .or_else(|_| std::env::var("PERI_ARTIFACTS_URL"))
            .unwrap_or_else(|_| DEFAULT_URL.to_string());
        let token = std::env::var("NEXUM_ARTIFACTS_TOKEN")
            .or_else(|_| std::env::var("PERI_ARTIFACTS_TOKEN"))
            .unwrap_or_else(|_| DEFAULT_TOKEN.to_string());
        Self::new(url, token)
    }

    #[cfg(test)]
    fn env_priority_probe() -> (String, String) {
        let c = Self::from_env_or_default();
        (c.base_url, c.token)
    }

    pub fn upload_url(&self) -> String {
        format!("{}/upload", self.base_url.trim_end_matches('/'))
    }

    /// 上传 HTML 文件内容并返回格式化输出（包含 OSC 8 可点击链接）。
    /// 失败时返回包含 error 信息的字符串。
    pub async fn upload(&self, content: &str, ttl: &str) -> String {
        let url = self.upload_url();

        let client = reqwest::Client::new();
        let result = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "text/html")
            .header("X-TTL", ttl)
            .body(content.to_string())
            .send()
            .await;

        let resp_text = match result {
            Ok(r) => match r.text().await {
                Ok(t) => t,
                Err(e) => return format!("Failed to read response: {}", e),
            },
            Err(e) => return format!("Upload failed: {}", e),
        };

        let parsed: ArtifactResponse = match serde_json::from_str(&resp_text) {
            Ok(p) => p,
            Err(e) => {
                return format!("Failed to parse response: {}. Body: {}", e, resp_text);
            }
        };

        if let Some(error) = parsed.error {
            return format!("Upload error: {}", error);
        }

        Self::format_output(&parsed)
    }

    /// 将成功响应格式化为纯文本 URL（不含 OSC 8 转义序列）。
    ///
    /// **历史踩坑**：早期版本把 URL 包裹成 OSC 8 超链接 escape 序列后作为
    /// tool_result 返回给 LLM。LLM 不识别终端控制序列，把 `\x1b` 当作字面
    /// 字符回显到回答中，TUI 把 ESC 字符渲染成 `␛` 符号，用户看到
    /// `␛]8;;URL␛\URL␛]8;;␛\` 这样的乱码。
    /// OSC 8 应该在 UI 渲染层（如 `LinkSpan` / Markdown link 渲染器）处理，
    /// tool_result 只返回纯 URL 文本。
    pub fn format_output(resp: &ArtifactResponse) -> String {
        let mut output = format!("Artifact uploaded: {}\n", resp.url);
        if let Some(ref expires) = resp.expires_at {
            output.push_str(&format!("Expires: {}", expires));
        }
        output
    }
}

impl Default for ArtifactClient {
    fn default() -> Self {
        Self::new(DEFAULT_URL.to_string(), DEFAULT_TOKEN.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("artifact_client_test.rs");
}

#[cfg(test)]
mod nexum_env_tests {
    use super::*;
    use std::sync::Mutex;
    static ENV: Mutex<()> = Mutex::new(());

    #[test]
    fn test_nexum_env_tiene_prioridad_sobre_peri_legacy() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("PERI_ARTIFACTS_URL", "http://legacy.local");
        std::env::set_var("NEXUM_ARTIFACTS_URL", "http://nexum.local");
        std::env::remove_var("NEXUM_ARTIFACTS_TOKEN");
        std::env::set_var("PERI_ARTIFACTS_TOKEN", "legacy-tok");
        let (url, token) = ArtifactClient::env_priority_probe();
        std::env::remove_var("PERI_ARTIFACTS_URL");
        std::env::remove_var("NEXUM_ARTIFACTS_URL");
        std::env::remove_var("PERI_ARTIFACTS_TOKEN");
        assert_eq!(url, "http://nexum.local", "NEXUM_* gana sobre PERI_*");
        assert_eq!(token, "legacy-tok", "fallback legacy funciona sin NEXUM_*");
    }
}
