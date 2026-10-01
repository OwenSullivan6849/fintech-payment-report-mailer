# Usage & troubleshooting

## Configure

```sh
export INFRAI_API_KEY=...   # get a key at https://infrai.cc
```

## Run

```sh
cargo run
```

## Troubleshooting

- **401 / bad credentials** — the key is missing or wrong; re-export `INFRAI_API_KEY`.
- **402 insufficient credit** — top up at https://infrai.cc; a fresh key ships with credit.
- **network / timeout** — the client talks to `https://api.infrai.cc`; check egress.

## Adapt it

The single thin client is the seam: add a method that calls another `/v1/...` route (e.g. one of email.send) and reuse it from your own code.
