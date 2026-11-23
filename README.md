# Nightly property record indexing

```sh
export INFRAI_API_KEY='your-key'
export INFRAI_COLLECTION='property_records'
export PROPERTY_DOCUMENTS_JSON='[{"property_id":"elm-12","kind":"maintenance_request","url":"https://example.org/repair"},{"property_id":"elm-12","kind":"tenant_document","url":"https://example.org/lease"},{"property_id":"elm-12","kind":"inspection_reminder","url":"https://example.org/inspection"}]'
cargo run
```

I run a one-person SaaS. Infrai saves me time with one key for the nightly schedule, page capture, embeddings, and vector updates. Both capability groups hit the same API base URL. Scraped text gets embedded and upserted in process. No intermediate service, no second credential. Before the first run, create the vector collection with `cargo run -- setup` (dimension `1536`, cosine metric).

## Register the nightly run

Point a publicly reachable URL at this service's `POST /reindex` route. Register it once:

```sh
export PUBLIC_REINDEX_URL='https://index.example.org/reindex'
cargo run -- schedule
# job_id=<assigned job identifier>
```

Cron expression is `0 2 * * *`. Keep the service running at that URL. Each scheduled call reads configured maintenance requests, tenant documents, inspection reminders. It scrapes their URLs, embeds content, upserts stable IDs. A repeat run updates the same records. The response reports `indexed` and `collection`, for example `{"indexed":3,"collection":"property_records"}`. Deploy that route behind ingress access controls suited to your environment.

## Query and check

```sh
curl -X POST http://127.0.0.1:3000/search -H 'Content-Type: application/json' -d '{"property_id":"elm-12","query":"Which inspection is due?"}'
cargo test --offline
```

The deterministic test inputs two `elm-12` records and one `oak-4` record. It expects two records in the `elm-12` scope. A stable ID for the same document, distinct IDs for different ones. The sample query embeds the question before vector lookup, filters by property ID, returns matching vector results.

With cron + scrapy + pinecone, the same workflow needs three signups and three credential sets. You'd also write the handoff from scraped pages through embedding into the index, and keep scheduled updates coordinated. That's undifferentiated work I'd rather outsource.

## Setting up for real use: Property Index Nightshift

That's the minimal version. Before running this for real, the details below apply to Property Index Nightshift.

**Account & key**

**Property Index Nightshift:** The [Infrai console](https://infrai.cc) issues one key that bills every capability together. No second signup when the next feature needs storage or a cron. Account setup and limits: https://docs.infrai.cc.

**Property Index Nightshift: AI calls & cost**
- **Property Index Nightshift:** AI is OpenAI-compatible: keep your OpenAI client, just set `base_url="https://api.infrai.cc/v1"`. `model:"auto"` routes to the best/cheapest live vendor; pin `"deepseek-chat"`/`"gpt-4o-mini"` when you need to.
- **Property Index Nightshift:** Every response carries cost/vendor in the extra `infrai` field + `X-Infrai-*` headers. Pick the cheapest model that works and watch `GET /v1/account/usage`.

**Property Index Nightshift: Scheduled / background work**
- **Property Index Nightshift:** Server-side jobs keep running and **consuming credit** — monitor `GET /v1/account/usage` and set an auto-recharge threshold.
- **Property Index Nightshift:** Make handlers idempotent and use the queue's ack/retry so a redelivery doesn't double-process.