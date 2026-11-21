const state = {
  startDate: "",
  endDate: "",
  clients: [],
  activeClient: "workbuddy",
};

function fmtNum(v, digits = 2) {
  return Number(v || 0).toLocaleString("zh-CN", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

function escHtml(v) {
  return String(v ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function safeHref(u) {
  const s = String(u ?? "").trim();
  return /^https?:\/\//i.test(s) ? escHtml(s) : "#";
}

function initDates() {
  const end = new Date();
  const start = new Date();
  start.setDate(end.getDate() - 29);
  const toIso = (d) => d.toISOString().slice(0, 10);
  state.startDate = toIso(start);
  state.endDate = toIso(end);
  document.getElementById("start-date").value = state.startDate;
  document.getElementById("end-date").value = state.endDate;
}

async function api(path, body) {
  const opts = body
    ? {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }
    : {};
  const res = await fetch(path, opts);
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

async function loadHealth() {
  try {
    const h = await api("/api/v1/health");
    const rows = h.storage?.unified_reports ?? 0;
    document.getElementById("health-pill").textContent =
      `在线 · ${h.plugins_mounted} 插件 · ${rows} 条报表`;
  } catch (e) {
    document.getElementById("health-pill").textContent = "连接失败";
  }
}

async function loadOverview() {
  const spendKey = document.getElementById("spend-caliber").value;
  const perfKey = document.getElementById("perf-caliber").value;
  const data = await api("/api/v1/analytics/reconcile", {
    start_date: state.startDate,
    end_date: state.endDate,
    group_by: ["platform"],
  });

  const t = data.reconciliation_total;
  const customKeys = Object.keys(t.custom_metrics || {});
  document.getElementById("custom-caliber-tags").textContent =
    customKeys.length ? `自定义口径: ${customKeys.join(" · ")}` : "";

  const kpis = [
    { label: "当前消耗口径", val: `¥${fmtNum(t[spendKey])}`, sub: `现金 ¥${fmtNum(t.cash_cost_yuan)} / 赠款 ¥${fmtNum(t.grant_cost_yuan)}` },
    { label: "当前业绩口径", val: `¥${fmtNum(t[perfKey])}`, sub: `代理净营收 ¥${fmtNum(t.agency_net_revenue_yuan)}` },
    { label: "账面 ROI / 全域 ROI", val: `${fmtNum(t.gross_roi)} / ${fmtNum(t.full_funnel_roi)}`, sub: `有效净ROI: ${fmtNum(t.custom_metrics?.effective_net_roi)}` },
    { label: "总曝光 / 总点击", val: `${Number(t.impressions).toLocaleString()} / ${Number(t.clicks).toLocaleString()}`, sub: `CTR ${(t.ctr * 100).toFixed(2)}% · CPC ¥${fmtNum(t.cpc_yuan)}` },
  ];
  document.getElementById("kpi-grid").innerHTML = kpis
    .map(
      (k) => `<div class="kpi"><div class="label">${k.label}</div><div class="val">${k.val}</div><div class="sub">${k.sub}</div></div>`
    )
    .join("");

  const tbody = document.querySelector("#platform-table tbody");
  tbody.innerHTML = data.groups
    .map(
      (g) => `<tr>
        <td><strong>${escHtml(g.label)}</strong></td>
        <td>¥${fmtNum(g[spendKey])}</td>
        <td>¥${fmtNum(g.cash_cost_yuan)} / ¥${fmtNum(g.grant_cost_yuan)}</td>
        <td>¥${fmtNum(g.kol_and_service_cost_yuan)}</td>
        <td>¥${fmtNum(g[perfKey])}</td>
        <td>${fmtNum(g.gross_roi)} / ${fmtNum(g.full_funnel_roi)}</td>
        <td>${(g.ctr * 100).toFixed(2)}% / ${(g.cvr * 100).toFixed(2)}%</td>
        <td>${fmtNum(g.custom_metrics?.effective_net_roi)}</td>
      </tr>`
    )
    .join("");
}

async function loadGovernance() {
  const [byType, byPerson, bindings] = await Promise.all([
    api("/api/v1/analytics/query", { start_date: state.startDate, end_date: state.endDate, group_by: ["project_type"] }),
    api("/api/v1/analytics/query", { start_date: state.startDate, end_date: state.endDate, group_by: ["person"] }),
    api("/api/v1/bindings"),
  ]);

  document.querySelector("#project-type-table tbody").innerHTML = byType.groups
    .map((g) => `<tr><td>${escHtml(g.label)}</td><td>¥${fmtNum(g.gross_media_cost_yuan)}</td><td>¥${fmtNum(g.cash_cost_yuan)}</td><td>¥${fmtNum(g.platform_gmv_yuan)}</td><td>${fmtNum(g.full_funnel_roi)}</td></tr>`)
    .join("");

  document.querySelector("#person-table tbody").innerHTML = byPerson.groups
    .map((g) => `<tr><td>${escHtml(g.label)}</td><td>¥${fmtNum(g.gross_media_cost_yuan)}</td><td>¥${fmtNum(g.agency_net_revenue_yuan)}</td><td>¥${fmtNum(g.platform_gmv_yuan)}</td><td>${fmtNum(g.gross_roi)}</td></tr>`)
    .join("");

  document.querySelector("#bindings-table tbody").innerHTML = bindings.bindings
    .map((b) => `<tr><td>${escHtml(b.id)}</td><td>${escHtml(b.person_id)}</td><td>${escHtml(b.ad_account_id)}</td><td>${escHtml(b.relation_role)}</td><td>${(b.split_weight * 100).toFixed(0)}%</td><td>${escHtml(b.effective_from)}</td><td>${escHtml(b.effective_to || "长期")}</td></tr>`)
    .join("");
}

async function loadCreatives() {
  const data = await api("/api/v1/attribution/creatives", {
    start_date: state.startDate,
    end_date: state.endDate,
  });

  document.querySelector("#creator-table tbody").innerHTML = data.creator_workload_summary
    .map((c) => `<tr><td><strong>${escHtml(c.person_name)}</strong></td><td>${escHtml(c.department)}</td><td>${c.assets_touched}</td><td>${c.total_work_hours}</td><td>¥${fmtNum(c.attributed_media_spend_fen / 100)}</td><td>¥${fmtNum(c.attributed_gmv_fen / 100)}</td><td><strong>¥${fmtNum(c.gmv_per_work_hour_yuan)}/h</strong></td><td>${fmtNum(c.avg_attributed_roi)}</td></tr>`)
    .join("");

  document.querySelector("#seeding-table tbody").innerHTML = data.seeding_boost_attribution
    .map((s) => `<tr><td><code>${escHtml(s.content_id)}</code></td><td>${escHtml(s.seeding_platform)}</td><td>${escHtml(s.boost_platform || "-")}</td><td>¥${fmtNum((s.kol_order_fee_fen + s.kol_service_fee_fen) / 100)}</td><td>¥${fmtNum(s.paid_boost_cost_fen / 100)}</td><td><strong>${fmtNum(s.full_funnel_roi)}</strong></td><td>¥${fmtNum(s.full_funnel_cpe_yuan)}</td></tr>`)
    .join("");

  document.querySelector("#creative-table tbody").innerHTML = data.creative_attribution
    .map((r) => `<tr><td>${escHtml(r.asset_code)}</td><td>${escHtml(r.format)} / ${escHtml(r.hook_type)}</td><td>${escHtml(r.scene_tag)}</td><td>${r.production_hours}h</td><td>¥${fmtNum(r.production_cost_fen / 100)}</td><td>¥${fmtNum(r.media_gross_cost_fen / 100)}</td><td>¥${fmtNum(r.gmv_fen / 100)}</td><td>${fmtNum(r.roi)}</td></tr>`)
    .join("");
}

async function loadPlatforms() {
  const data = await api("/api/v1/platforms");
  document.querySelector("#connectors-table tbody").innerHTML = data.platforms
    .map((item) => `<tr><td><strong>${escHtml(item.meta.name_zh)}</strong><br/><span style="color:#93a4c3">${escHtml(item.meta.id)}</span></td><td>${escHtml(item.meta.access_mode)}</td><td>${escHtml(item.mode)}</td><td>${item.meta.max_sync_days_per_chunk} 天</td><td>${item.meta.default_qps}</td><td><a style="color:#4f8cff" target="_blank" rel="noopener noreferrer" href="${safeHref(item.meta.docs_url)}">${escHtml(item.meta.base_url)}</a></td></tr>`)
    .join("");
}

async function loadAgents() {
  const schema = await api("/api/v1/ui/schema");
  state.clients = schema.supported_clients || [];
  renderAgents();
}

function renderAgents() {
  const host = window.location.origin;
  const pills = document.getElementById("agent-pills");
  pills.innerHTML = state.clients
    .map((c) => `<button class="agent-pill ${c.id === state.activeClient ? "active" : ""}" data-client="${escHtml(c.id)}">${escHtml(c.name)}</button>`)
    .join("");

  pills.querySelectorAll(".agent-pill").forEach((btn) => {
    btn.onclick = () => {
      state.activeClient = btn.dataset.client;
      renderAgents();
    };
  });

  const c = state.activeClient;
  const isCodex = c === "codex";
  const cfg = isCodex
    ? `[mcp_servers.adlink]\nurl = "${host}/mcp"\n# 或本地 Stdio 模式:\n# command = "adlink"\n# args = ["mcp-stdio", "--config", "config/adlink.mock.toml"]`
    : JSON.stringify(
        {
          mcpServers: {
            adlink: {
              url: `${host}/mcp`,
              legacySseUrl: `${host}/sse`,
              stdioCommand: "adlink",
              stdioArgs: ["mcp-stdio", "--config", "config/adlink.mock.toml"],
            },
          },
        },
        null,
        2
      );
  document.getElementById("agent-config-preview").textContent =
    `# 目标客户端: ${c}\n# Streamable HTTP: ${host}/mcp | Legacy SSE: ${host}/sse\n\n${cfg}`;
}

async function refreshAll() {
  state.startDate = document.getElementById("start-date").value;
  state.endDate = document.getElementById("end-date").value;
  await Promise.all([loadHealth(), loadOverview(), loadGovernance(), loadCreatives(), loadPlatforms(), loadAgents()]);
}

document.querySelectorAll(".nav-btn").forEach((btn) => {
  btn.onclick = () => {
    document.querySelectorAll(".nav-btn").forEach((b) => b.classList.remove("active"));
    document.querySelectorAll(".tab-panel").forEach((p) => p.classList.remove("active"));
    btn.classList.add("active");
    document.getElementById(`tab-${btn.dataset.tab}`).classList.add("active");
    document.getElementById("page-title").textContent = btn.textContent;
  };
});

document.getElementById("refresh-btn").onclick = refreshAll;
document.getElementById("spend-caliber").onchange = loadOverview;
document.getElementById("perf-caliber").onchange = loadOverview;
document.getElementById("sync-btn").onclick = async () => {
  const btn = document.getElementById("sync-btn");
  btn.textContent = "并发拉取中...";
  try {
    await api("/api/v1/sync/reports", {
      start_date: state.startDate,
      end_date: state.endDate,
      levels: ["account", "campaign", "creative", "content_order"],
      max_concurrency: 12,
    });
    await refreshAll();
  } finally {
    btn.textContent = "一键触发并发同步 (30天)";
  }
};

initDates();
refreshAll().catch(console.error);
