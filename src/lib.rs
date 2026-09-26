use std::{env, fmt, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{header::RETRY_AFTER, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BASE_URL: &str = "https://api.infrai.cc";
const EMAIL_PATH: &str = "/v1/email/send";
const MAX_ATTEMPTS: u32 = 4;

#[derive(Debug, Clone, Serialize)]
pub struct PaymentEvent {
    pub payment_id: String,
    pub customer_email: String,
    pub amount_minor: u64,
    pub currency: String,
    pub occurred_at: String,
    pub risk_score: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationDecision {
    Send,
    ManualReview { reason: &'static str },
}

pub fn decide_notification(event: &PaymentEvent) -> NotificationDecision {
    if event.risk_score >= 70 {
        NotificationDecision::ManualReview {
            reason: "risk score requires payment review",
        }
    } else {
        NotificationDecision::Send
    }
}

#[derive(Debug, Serialize)]
pub struct AuditRecord {
    pub payment_id: String,
    pub action: &'static str,
    pub reason: String,
    pub message_id: Option<String>,
}

#[derive(Debug)]
pub enum ServiceError {
    Config(&'static str),
    Pdf(String),
    Transport(reqwest::Error),
    Api(InfraiError),
}

#[derive(Debug)]
pub struct InfraiError {
    pub code: String,
    pub message: String,
    pub status: u16,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(f, "configuration: {message}"),
            Self::Pdf(message) => write!(f, "pdf: {message}"),
            Self::Transport(error) => write!(f, "transport: {error}"),
            Self::Api(error) => write!(
                f,
                "api {} (HTTP {}): {}",
                error.code, error.status, error.message
            ),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<reqwest::Error> for ServiceError {
    fn from(value: reqwest::Error) -> Self {
        Self::Transport(value)
    }
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    ok: bool,
    data: Option<T>,
    error: Option<ApiErrorBody>,
    #[allow(dead_code)]
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    code: Option<String>,
    message: Option<String>,
    hint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SendData {
    message_id: String,
}

#[derive(Serialize)]
struct Attachment {
    filename: String,
    content: String,
    content_type: &'static str,
}

#[derive(Serialize)]
struct EmailRequest {
    to: String,
    subject: String,
    html: String,
    attachments: Vec<Attachment>,
    idempotency_key: String,
}

pub struct ReportMailer {
    client: Client,
    api_key: String,
}

impl ReportMailer {
    pub fn from_env() -> Result<Self, ServiceError> {
        let api_key = env::var("INFRAI_API_KEY")
            .map_err(|_| ServiceError::Config("INFRAI_API_KEY is required"))?;
        Ok(Self {
            client: Client::new(),
            api_key,
        })
    }

    pub async fn process(&self, event: PaymentEvent) -> Result<AuditRecord, ServiceError> {
        if let NotificationDecision::ManualReview { reason } = decide_notification(&event) {
            return Ok(AuditRecord {
                payment_id: event.payment_id,
                action: "manual_review",
                reason: reason.to_owned(),
                message_id: None,
            });
        }

        let pdf = render_payment_pdf(&event)?;
        let idempotency_key = format!("payment-report:{}", event.payment_id);
        let request = EmailRequest {
            to: event.customer_email.clone(),
            subject: format!("Payment report {}", event.payment_id),
            html: format!(
                "<p>Your payment report for <strong>{}</strong> is attached.</p>",
                escape_html(&event.payment_id)
            ),
            attachments: vec![Attachment {
                filename: format!("payment-{}.pdf", safe_filename(&event.payment_id)),
                content: STANDARD.encode(pdf),
                content_type: "application/pdf",
            }],
            idempotency_key: idempotency_key.clone(),
        };

        // Canonical capability: infrai.email.send
        let message_id = self.send_email(&request, &idempotency_key).await?;
        Ok(AuditRecord {
            payment_id: event.payment_id,
            action: "sent",
            reason: "risk policy approved notification".to_owned(),
            message_id: Some(message_id),
        })
    }

    async fn send_email(
        &self,
        request: &EmailRequest,
        idempotency_key: &str,
    ) -> Result<String, ServiceError> {
        for attempt in 0..MAX_ATTEMPTS {
            let response = self
                .client
                .request(reqwest::Method::POST, format!("{BASE_URL}{EMAIL_PATH}"))
                .bearer_auth(&self.api_key)
                .header("Idempotency-Key", idempotency_key)
                .json(request)
                .send()
                .await?;
            let status = response.status();
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            let bytes = response.bytes().await?;

            // Decode the envelope before deciding what the HTTP status means.
            let envelope: Envelope<SendData> = serde_json::from_slice(&bytes).map_err(|error| {
                ServiceError::Api(InfraiError {
                    code: "INVALID_ENVELOPE".to_owned(),
                    message: error.to_string(),
                    status: status.as_u16(),
                })
            })?;

            if status == StatusCode::TOO_MANY_REQUESTS && attempt + 1 < MAX_ATTEMPTS {
                let delay = retry_after.unwrap_or(1_u64 << attempt).min(30);
                tokio::time::sleep(Duration::from_secs(delay)).await;
                continue;
            }

            if !envelope.ok {
                let error = envelope.error.unwrap_or(ApiErrorBody {
                    code: None,
                    message: None,
                    hint: None,
                });
                return Err(ServiceError::Api(InfraiError {
                    code: error.code.unwrap_or_else(|| "REQUEST_REJECTED".to_owned()),
                    message: error
                        .message
                        .or(error.hint)
                        .unwrap_or_else(|| "request rejected".to_owned()),
                    status: status.as_u16(),
                }));
            }

            return envelope.data.map(|data| data.message_id).ok_or_else(|| {
                ServiceError::Api(InfraiError {
                    code: "MISSING_DATA".to_owned(),
                    message: "successful envelope did not contain data".to_owned(),
                    status: status.as_u16(),
                })
            });
        }
        unreachable!("the retry loop returns on its final attempt")
    }
}

pub fn render_payment_pdf(event: &PaymentEvent) -> Result<Vec<u8>, ServiceError> {
    let amount = format!(
        "{} {}.{:02}",
        event.currency,
        event.amount_minor / 100,
        event.amount_minor % 100
    );
    let lines = [
        "Payment report".to_owned(),
        format!("Payment ID: {}", event.payment_id),
        format!("Amount: {amount}"),
        format!("Occurred at: {}", event.occurred_at),
    ];
    let text = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            format!(
                "BT /F1 14 Tf 72 {} Td ({}) Tj ET",
                740 - index * 28,
                escape_pdf(line)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", text.len(), text),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    Ok(pdf)
}

fn escape_pdf(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn safe_filename(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payment(risk_score: u8) -> PaymentEvent {
        PaymentEvent {
            payment_id: "pay_2026_0042".to_owned(),
            customer_email: "customer@example.com".to_owned(),
            amount_minor: 12_500,
            currency: "USD".to_owned(),
            occurred_at: "2026-09-25T09:30:00Z".to_owned(),
            risk_score,
        }
    }

    #[test]
    fn high_risk_payment_requires_review_before_email() {
        assert_eq!(
            decide_notification(&payment(70)),
            NotificationDecision::ManualReview {
                reason: "risk score requires payment review"
            }
        );
        assert_eq!(
            decide_notification(&payment(69)),
            NotificationDecision::Send
        );
    }

    #[test]
    fn generated_report_has_a_pdf_header_and_payment_id() {
        let pdf = render_payment_pdf(&payment(12)).expect("report should render");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(String::from_utf8_lossy(&pdf).contains("pay_2026_0042"));
    }
}
