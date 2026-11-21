# AdLink MiniProgram / Multi-End SDK (`adlink-mp.js`)

AdLink treats the Web Console and MiniProgram GUI as **pluggable, user-customizable frontends** built atop:
1. **Schema-Driven UI Contract**: `GET /api/v1/ui/schema`
2. **Headless REST API**: `/api/v1/*` (documented in OpenAPI 3.1 at `/api/v1/openapi.json`)

## Example (WeChat MiniProgram / UniApp / Taro)

```js
import { AdLinkMiniProgramClient } from "./adlink-mp";

const client = new AdLinkMiniProgramClient({
  baseUrl: "https://your-adlink-domain.com",
  apiKey: "your-bearer-key",
});

// 1. Dynamically load caliber options & platform navigation from backend schema
const schema = await client.getUiSchema();

// 2. Fetch Spend / Performance / Custom caliber reconciliation
const report = await client.reconcileCalibers({
  startDate: "2026-11-01",
  endDate: "2026-11-21",
});

// 3. Fetch Creator Workload & Cross-Platform Seeding ROI
const creativeStats = await client.getCreativeAttribution({
  startDate: "2026-11-01",
  endDate: "2026-11-21",
});
```

To replace the built-in Web GUI with your own React/Next/Vue build, pass `--web-dir ./dist` to `adlink serve` or mount your directory over `/app/web-starter` in `docker-compose.yml`.
