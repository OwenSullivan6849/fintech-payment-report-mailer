use fintech_report_mailer::{PaymentEvent, ReportMailer, ServiceError};

#[tokio::main]
async fn main() -> Result<(), ServiceError> {
    let recipient = std::env::var("REPORT_RECIPIENT")
        .map_err(|_| ServiceError::Config("REPORT_RECIPIENT is required"))?;
    let event = PaymentEvent {
        payment_id: "pay_2026_0042".to_owned(),
        customer_email: recipient,
        amount_minor: 12_500,
        currency: "USD".to_owned(),
        occurred_at: "2026-09-25T09:30:00Z".to_owned(),
        risk_score: 18,
    };

    let audit = ReportMailer::from_env()?.process(event).await?;
    println!(
        "{}",
        serde_json::to_string(&audit).expect("audit record serializes")
    );
    Ok(())
}
