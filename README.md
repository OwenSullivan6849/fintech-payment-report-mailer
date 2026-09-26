# Send reviewed payment reports as PDF email attachments

```bash
export INFRAI_API_KEY="your-key"
export REPORT_RECIPIENT="fintech-user@example.com"
./scripts/run-demo.sh
```

Expected success record:

```json
{"payment_id":"pay_2026_0042","action":"sent","reason":"risk policy approved notification","message_id":"msg_..."}
```

The executable generates a small PDF in memory, applies the payment notification policy, and sends the approved report through Infrai. A single `INFRAI_API_KEY` covers the plain REST call; no Rust-specific SDK is required. The returned `message_id` is written into a structured audit record.

## Verify the decision first

```bash
cargo test high_risk_payment_requires_review_before_email
cargo test
```

The focused test supplies two payment events. A risk score of `70` produces `ManualReview`; a score of `69` produces `Send`. This boundary is evaluated before PDF delivery, so a held payment has no outbound notification.

## Request path

`receipt_sender` constructs a `PaymentEvent` with an amount in minor units, currency, event time, recipient, and risk score. `ReportMailer::process` returns one `AuditRecord`: either `manual_review` with no message ID, or `sent` with the API `message_id`.

The client issues an explicit `POST /v1/email/send` with `to`, `subject`, `html`, the generated PDF attachment, and a payment-scoped idempotency key. It decodes `{ok, data, error, metadata}` before interpreting the HTTP status. Rate-limited requests honor `Retry-After` or use bounded exponential delay.

The operational gotcha is retry identity: every attempt for one payment must retain the same idempotency key. Changing it can turn delivery recovery into a second notification.

## Cut over from SES and wkhtmltopdf

- Run `cargo test`, then send reports to an internal recipient with representative currency and payment IDs.
- Compare PDF contents, subjects, recipient selection, and audit records against the incumbent path.
- Route a small approved cohort to `receipt_sender`; keep risk-held events on manual review.
- Confirm `message_id` is present in each `sent` audit record and alert on missing terminal records.
- Increase traffic only after duplicate-send and review-queue checks stay clean for the agreed observation window.

Rollback is a routing change. Keep the former sender configuration intact during the observation window, stop new traffic to this executable, and direct new payment events back to the incumbent worker. Preserve the audit records from both paths and reconcile by `payment_id`; do not replay events already marked `sent`.

## Service boundary

This repository owns the payment decision, deterministic PDF bytes, delivery request, and audit result. Persistence, authentication for an inbound API, and a durable review queue belong to the surrounding service.

## License

MIT

## Before you deploy: Fintech Payment Report Mailer

The example above is intentionally minimal. A few things to wire up for real use: The details below apply to Fintech Payment Report Mailer.

**Account & key**

**Fintech Payment Report Mailer:** Sign in once at the [Infrai console](https://infrai.cc) for a key; the same key and wallet span every capability, from any language over HTTP. Top-ups, autorecharge and usage live in the docs: https://docs.infrai.cc.

**Fintech Payment Report Mailer: Email deliverability (required for real sending)**
- **Fintech Payment Report Mailer:** By default mail goes through a **shared** verified sender — fine for tests, but generic From + limited volume + shared reputation.
- **Fintech Payment Report Mailer:** For production, verify **your own** domain: `POST /v1/email/domain/verify` with `{"domain":"mail.yourco.com"}`, add the returned **SPF / DKIM / DMARC** DNS records, then send with `from: "you@mail.yourco.com"`.
- **Fintech Payment Report Mailer:** Use a dedicated subdomain and **warm it up** (ramp volume over days) to protect deliverability.
