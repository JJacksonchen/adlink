/**
 * AdLink MiniProgram / UniApp / Taro Headless Client SDK (Apache-2.0)
 * Connects WeChat MiniPrograms, ByteDance MiniPrograms, or Taro/UniApp GUIs
 * directly to the AdLink REST API (`/api/v1/*`) and Schema-Driven UI (`/api/v1/ui/schema`).
 */

export class AdLinkMiniProgramClient {
  constructor({ baseUrl, apiKey = "", requestFn = null }) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.apiKey = apiKey;
    // Defaults to global `wx.request` or `uni.request` when available.
    this.requestFn =
      requestFn ||
      ((opts) =>
        new Promise((resolve, reject) => {
          const runtime = typeof wx !== "undefined" ? wx : typeof uni !== "undefined" ? uni : null;
          if (!runtime) {
            reject(new Error("Provide requestFn when outside wx/uni MiniProgram runtime"));
            return;
          }
          runtime.request({
            ...opts,
            success: (res) => {
              if (res.statusCode >= 200 && res.statusCode < 300) resolve(res.data);
              else reject(new Error(`HTTP ${res.statusCode}: ${JSON.stringify(res.data)}`));
            },
            fail: reject,
          });
        }));
  }

  _request(path, method = "GET", data = undefined) {
    const header = { "Content-Type": "application/json" };
    if (this.apiKey) header["Authorization"] = `Bearer ${this.apiKey}`;
    return this.requestFn({
      url: `${this.baseUrl}${path}`,
      method,
      header,
      data,
    });
  }

  getUiSchema() {
    return this._request("/api/v1/ui/schema");
  }

  getHealth() {
    return this._request("/api/v1/health");
  }

  getPlatforms() {
    return this._request("/api/v1/platforms");
  }

  reconcileCalibers({ startDate, endDate, platforms = [], projectIds = [] }) {
    return this._request("/api/v1/analytics/reconcile", "POST", {
      start_date: startDate,
      end_date: endDate,
      platforms,
      project_ids: projectIds,
      group_by: ["platform"],
    });
  }

  queryByPerson({ startDate, endDate }) {
    return this._request("/api/v1/analytics/query", "POST", {
      start_date: startDate,
      end_date: endDate,
      group_by: ["person"],
    });
  }

  getCreativeAttribution({ startDate, endDate, projectId = null }) {
    return this._request("/api/v1/attribution/creatives", "POST", {
      start_date: startDate,
      end_date: endDate,
      project_id: projectId,
    });
  }

  triggerSync({ startDate, endDate, platforms = [], maxConcurrency = 8 }) {
    return this._request("/api/v1/sync/reports", "POST", {
      start_date: startDate,
      end_date: endDate,
      platforms,
      max_concurrency: maxConcurrency,
    });
  }

  mutateAd({ platform, adAccountId, entityLevel, entityIds, action, dryRun = true, reason }) {
    return this._request("/api/v1/ad-ops/mutate", "POST", {
      platform,
      ad_account_id: adAccountId,
      entity_level: entityLevel,
      entity_ids: entityIds,
      action,
      dry_run: dryRun,
      reason,
      operator: "miniprogram-user",
    });
  }
}
