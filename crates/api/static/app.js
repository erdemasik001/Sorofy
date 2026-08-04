/* Sorofy explorer — hash-routed, dependency-free.
 *
 * Everything rendered here originates from a stranger's submission (contract
 * ids, source URIs, and above all the build log, which is the output of
 * compiling code we did not write). So every interpolated value goes through
 * esc(), and the build log is written with textContent rather than markup.
 * Treat that as load-bearing, not stylistic.
 */
(function () {
  "use strict";

  var PAGE = 24;
  var view = document.getElementById("view");

  /* ── helpers ───────────────────────────────────────────────────────────── */

  function esc(value) {
    if (value === null || value === undefined) return "";
    return String(value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function get(path) {
    return fetch(path, { headers: { Accept: "application/json" } }).then(function (r) {
      if (r.status === 404) return null;
      if (!r.ok) throw new Error("Request failed (" + r.status + ")");
      return r.json();
    });
  }

  function ago(iso) {
    var then = Date.parse(iso);
    if (isNaN(then)) return iso || "";
    var secs = Math.max(0, (Date.now() - then) / 1000);
    if (secs < 60) return Math.floor(secs) + "s ago";
    if (secs < 3600) return Math.floor(secs / 60) + "m ago";
    if (secs < 86400) return Math.floor(secs / 3600) + "h ago";
    return Math.floor(secs / 86400) + "d ago";
  }

  function shortHash(hex) {
    if (!hex || hex.length <= 20) return hex || "";
    return hex.slice(0, 10) + "…" + hex.slice(-8);
  }

  function bytes(n) {
    if (typeof n !== "number") return "—";
    if (n < 1024) return n + " B";
    return (n / 1024).toFixed(1) + " KiB";
  }

  function seconds(n) {
    if (typeof n !== "number") return "—";
    return n < 10 ? n.toFixed(1) + "s" : Math.round(n) + "s";
  }

  /* Status vocabulary. `mismatch` is a verdict against the contract, so it is
   * the loud one; `error` is our infrastructure failing and must not look like
   * an accusation. */
  var STATUS = {
    verified: {
      tone: "ok",
      label: "Verified",
      title: "Verified",
      blurb: "The rebuilt WASM matches the hash recorded on chain, byte for byte.",
    },
    mismatch: {
      tone: "danger",
      label: "Mismatch",
      title: "Mismatch",
      blurb: "The rebuild completed but produced different bytecode from the contract on chain. The deployed code is not this source built this way.",
    },
    pending: {
      tone: "warn",
      label: "Pending",
      title: "Rebuild running",
      blurb: "The source is being rebuilt in the pinned container. This page reflects the cache the moment it was loaded.",
    },
    error: {
      tone: "muted",
      label: "Error",
      title: "Could not complete",
      blurb: "The job did not finish, so there is no verdict about the contract either way.",
    },
  };

  function statusOf(row) {
    return STATUS[row && row.status] || STATUS.error;
  }

  function icon(name, color, size) {
    var s = size || 30;
    var head =
      '<svg width="' + s + '" height="' + s + '" viewBox="0 0 24 24" fill="none" ' +
      'stroke="' + color + '" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">';
    var body = {
      check: '<path d="M20 6 9 17l-5-5"/>',
      x: '<path d="M18 6 6 18M6 6l12 12"/>',
      clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
      alert: '<path d="M12 9v4M12 17h.01"/><circle cx="12" cy="12" r="9"/>',
      arrow: '<path d="m15 18-6-6 6-6"/>',
      spin: '<path d="M21 12a9 9 0 1 1-6.2-8.6"/>',
    }[name] || "";
    return head + body + "</svg>";
  }

  function iconFor(status) {
    if (status === "verified") return icon("check", "var(--ok)");
    if (status === "mismatch") return icon("x", "var(--danger)");
    if (status === "pending") return '<span class="spin" style="display:inline-block">' + icon("spin", "var(--warn)") + "</span>";
    return icon("alert", "var(--ink-3)");
  }

  function pill(row) {
    var s = statusOf(row);
    return '<span class="brut-pill" data-tone="' + s.tone + '"><span class="dot"></span>' + esc(s.label) + "</span>";
  }

  function sourceSummary(source) {
    if (!source || typeof source !== "object") return { kind: "—", value: "—" };
    if (source.kind === "git") {
      return { kind: "Git", value: (source.repo || "") + "\n@ " + (source.rev || "") };
    }
    if (source.kind === "archive") {
      return { kind: "Archive", value: source.uri || "" };
    }
    return { kind: source.kind || "—", value: "—" };
  }

  function stateBlock(iconName, color, title, body) {
    return (
      '<div class="brut-card state">' +
      icon(iconName, color, 32) +
      "<h3>" + esc(title) + "</h3>" +
      "<p>" + esc(body) + "</p>" +
      "</div>"
    );
  }

  function setNav(which) {
    var links = document.querySelectorAll("[data-nav]");
    for (var i = 0; i < links.length; i++) {
      if (links[i].getAttribute("data-nav") === which) links[i].setAttribute("aria-current", "page");
      else links[i].removeAttribute("aria-current");
    }
  }

  /* ── home ──────────────────────────────────────────────────────────────── */

  function hero() {
    return (
      '<section class="hero fade-up">' +
      '<span class="hero__badge mono"><span class="dot"></span>Live on Stellar Testnet</span>' +
      "<h1>Don't trust the bytecode.<br>" +
      '<span class="sticker">Rebuild it.</span></h1>' +
      '<p class="hero__sub">A contract on chain is just bytecode. Sorofy rebuilds the source in a ' +
      "digest-pinned container and compares the result against the hash the network itself reports — " +
      "so the match is evidence, not a claim.</p>" +
      '<form class="searchbar" id="search">' +
      '<input class="brut-input" id="q" placeholder="Contract ID, WASM hash, or job id" ' +
      'aria-label="Contract ID, WASM hash, or job id" spellcheck="false" autocomplete="off">' +
      '<button class="brut-btn brut-btn-accent" type="submit">Look up</button>' +
      "</form>" +
      '<p class="searchbar__hint">Reads the cache. Starting a new verification is a ' +
      "<span class=\"mono\">POST /verify</span> and needs a token.</p>" +
      "</section>"
    );
  }

  function card(row) {
    var report = row.report || {};
    var key = row.contract_id || row.wasm_hash || row.id;
    return (
      '<a class="brut-card vcard" href="#/v/' + encodeURIComponent(key) + '">' +
      '<div class="vcard__top">' + pill(row) + '<span class="vcard__time">' + esc(ago(row.created_at)) + "</span></div>" +
      '<div class="vcard__body">' +
      "<div>" +
      '<div class="label">' + (row.contract_id ? "Contract" : "WASM hash") + "</div>" +
      '<div class="vcard__id">' + esc(row.contract_id || shortHash(row.wasm_hash)) + "</div>" +
      (row.contract_id ? '<div class="vcard__hash">' + esc(shortHash(row.wasm_hash)) + "</div>" : "") +
      "</div>" +
      '<div class="stat-2up">' +
      '<div><div class="label">Size</div><div class="v">' + esc(bytes(report.rebuilt_wasm_size)) + "</div></div>" +
      '<div><div class="label">Build</div><div class="v">' + esc(seconds(report.build_seconds)) + "</div></div>" +
      "</div>" +
      "</div></a>"
    );
  }

  function renderHome(offset) {
    setNav("home");
    view.innerHTML = '<div class="wrap">' + hero() + '<div id="list"></div></div>';
    wireSearch();

    var list = document.getElementById("list");
    list.innerHTML = '<div class="brut-card state"><span class="spin" style="display:inline-block">' +
      icon("spin", "var(--accent)", 32) + "</span><h3>Loading…</h3></div>";

    get("/verifications?limit=" + PAGE + "&offset=" + offset)
      .then(function (data) {
        var items = (data && data.items) || [];
        var total = (data && data.total) || 0;
        if (!items.length) {
          list.innerHTML =
            '<section class="section"><div class="section__head"><h2>Recent verifications</h2>' +
            '<span class="counter mono">00</span></div>' +
            stateBlock("clock", "var(--ink-3)", "Nothing verified yet",
              "Once a verification job runs, every result lands here — verified or not.") +
            "</section>";
          return;
        }
        var html =
          '<section class="section"><div class="section__head"><h2>Recent verifications</h2>' +
          '<span class="counter mono">' + esc(String(total).padStart(2, "0")) + "</span></div>" +
          '<div class="cardgrid">' + items.map(card).join("") + "</div>";

        if (total > PAGE) {
          var from = offset + 1;
          var to = offset + items.length;
          html +=
            '<div class="pager">' +
            (offset > 0
              ? '<a class="brut-btn brut-btn-ghost" href="#/p/' + Math.max(0, offset - PAGE) + '">Newer</a>'
              : "") +
            '<span class="pager__at">' + from + "–" + to + " of " + total + "</span>" +
            (to < total ? '<a class="brut-btn brut-btn-ghost" href="#/p/' + (offset + PAGE) + '">Older</a>' : "") +
            "</div>";
        }
        list.innerHTML = html + "</section>";
      })
      .catch(function (err) {
        list.innerHTML = stateBlock("alert", "var(--danger)", "Could not reach the API", err.message);
      });
  }

  function wireSearch() {
    var form = document.getElementById("search");
    if (!form) return;
    form.addEventListener("submit", function (e) {
      e.preventDefault();
      var q = document.getElementById("q").value.trim();
      if (q) location.hash = "#/v/" + encodeURIComponent(q);
    });
  }

  /* ── detail ────────────────────────────────────────────────────────────── */

  function tile(label, value) {
    return '<div class="brut-tile"><div class="label">' + esc(label) + '</div><div class="v">' + esc(value) + "</div></div>";
  }

  function renderDetail(key) {
    setNav("");
    view.innerHTML =
      '<div class="wrap wrap--narrow"><a class="backlink" href="#/">' +
      icon("arrow", "currentColor", 15) + "Back to explorer</a><div id=\"d\"></div></div>";
    var d = document.getElementById("d");
    d.innerHTML = '<div class="brut-card state"><span class="spin" style="display:inline-block">' +
      icon("spin", "var(--accent)", 32) + "</span><h3>Loading…</h3></div>";

    get("/verify/" + encodeURIComponent(key))
      .then(function (row) {
        if (!row) {
          d.innerHTML = stateBlock("clock", "var(--ink-3)", "No record",
            "Nothing in the cache matches “" + key + "”. Only contracts that have been submitted for verification appear here.");
          return;
        }
        d.innerHTML = detailHtml(row);
        // The build log is compiler output from source we did not write, so it
        // goes in as text, never as markup. Done here rather than in a second
        // pass so there is no window where the element exists without it.
        var pre = document.getElementById("log");
        if (pre && row.report && row.report.build_log) pre.textContent = row.report.build_log;
      })
      .catch(function (err) {
        d.innerHTML = stateBlock("alert", "var(--danger)", "Could not load the record", err.message);
      });
  }

  function detailHtml(row) {
    var s = statusOf(row);
    var report = row.report || {};
    var matched = row.status === "verified";
    var src = sourceSummary(row.source);
    var html = "";

    html +=
      '<div class="brut-card-2 status-panel fade-up">' +
      '<div class="status-panel__icon">' + iconFor(row.status) + "</div>" +
      "<div><h2>" + esc(s.title) + "</h2><p>" + esc(row.status === "error" && row.error ? row.error : s.blurb) + "</p>" +
      '<div style="margin-top:14px;display:flex;gap:8px;flex-wrap:wrap">' + pill(row) +
      (report.trust_level ? '<span class="brut-pill" data-tone="muted">trust: ' + esc(report.trust_level) + "</span>" : "") +
      '<span class="brut-pill" data-tone="muted">job #' + esc(row.id) + "</span></div></div></div>";

    if (report.expected_wasm_sha256 || report.rebuilt_wasm_sha256) {
      html +=
        '<div class="brut-card hashcmp" data-match="' + (matched ? "yes" : "no") + '">' +
        '<div class="hashcmp__row"><div class="label">On chain (expected)</div>' +
        '<div class="hash">' + esc(report.expected_wasm_sha256) + "</div></div>" +
        '<div class="hashcmp__row"><div class="label">Rebuilt here</div>' +
        '<div class="hash">' + esc(report.rebuilt_wasm_sha256) + "</div></div>" +
        '<div class="hashcmp__verdict">' +
        (matched ? icon("check", "var(--ok)", 20) : icon("x", "var(--danger)", 20)) +
        (matched ? "Byte-for-byte identical" : "These are different bytes") +
        "</div></div>";
    }

    html += '<div class="tilegrid">';
    if (row.contract_id) html += tile("Contract", row.contract_id);
    html += tile("Target WASM hash", row.wasm_hash);
    html += tile("Source", src.kind + " — " + src.value);
    if (report.source_sha256) html += tile("Source sha256", report.source_sha256);
    html += tile("Build image", report.bldimg_digest || row.bldimg);
    if (report.artifact) html += tile("Artifact", report.artifact);
    if (typeof report.rebuilt_wasm_size === "number") html += tile("Rebuilt size", bytes(report.rebuilt_wasm_size));
    if (typeof report.build_seconds === "number") html += tile("Build time", seconds(report.build_seconds));
    if (report.bldopt && report.bldopt.length) html += tile("Build options", report.bldopt.join(" "));
    html += tile("Recorded", row.created_at + " · updated " + row.updated_at);
    html += "</div>";

    if (report.build_log) {
      html += '<div class="brut-card logbox"><div class="label">Build log</div><pre id="log"></pre></div>';
    }
    return html;
  }

  /* ── metrics ───────────────────────────────────────────────────────────── */

  function metricTile(label, value, note) {
    return (
      '<div class="brut-tile"><div class="label">' + esc(label) + "</div>" +
      '<div class="metric">' + esc(value) + "</div>" +
      (note ? '<div class="metric__note">' + esc(note) + "</div>" : "") +
      "</div>"
    );
  }

  function renderMetrics() {
    setNav("metrics");
    view.innerHTML =
      '<div class="wrap wrap--narrow"><div class="section__head"><h2>Service metrics</h2></div>' +
      '<div id="m"></div></div>';
    var m = document.getElementById("m");
    m.innerHTML = '<div class="brut-card state"><span class="spin" style="display:inline-block">' +
      icon("spin", "var(--accent)", 32) + "</span><h3>Loading…</h3></div>";

    // Health is allowed to answer 503 (degraded), which is information rather
    // than a failure — so it is read directly instead of through get().
    Promise.all([
      get("/metrics"),
      fetch("/health", { headers: { Accept: "application/json" } })
        .then(function (r) { return r.json().then(function (b) { return { ok: r.ok, body: b }; }); })
        .catch(function () { return { ok: false, body: { status: "unreachable" } }; }),
    ])
      .then(function (results) {
        var d = results[0] || {};
        var h = results[1];
        var healthy = h.ok && h.body && h.body.status === "ok";

        var html =
          '<div class="brut-card-2 status-panel fade-up">' +
          '<div class="status-panel__icon">' +
          (healthy ? icon("check", "var(--ok)") : icon("alert", "var(--danger)")) + "</div>" +
          "<div><h2>" + (healthy ? "Service healthy" : "Service degraded") + "</h2>" +
          "<p>" + (healthy
            ? "The API is up and the verification cache answers queries."
            : "The API responded, but its cache did not: " + esc(h.body && h.body.error ? h.body.error : h.body.status)) +
          "</p></div></div>";

        html += '<div class="tilegrid">' +
          metricTile("Jobs submitted", d.jobs_submitted, "since this process started") +
          metricTile("In flight", d.jobs_in_flight, "rebuilding right now") +
          metricTile("Verified", d.verified) +
          metricTile("Mismatch", d.mismatch) +
          metricTile("Errored", d.error) +
          metricTile("Avg build", seconds(d.avg_build_seconds), "per completed rebuild") +
          "</div>";

        html += '<p class="searchbar__hint">Counters are per process and reset on restart — ' +
          "they measure this instance, not the contract history, which lives in the cache.</p>";
        m.innerHTML = html;
      })
      .catch(function (err) {
        m.innerHTML = stateBlock("alert", "var(--danger)", "Could not read metrics", err.message);
      });
  }

  /* ── routing ───────────────────────────────────────────────────────────── */

  function route() {
    var hash = location.hash || "#/";
    var detail = hash.match(/^#\/v\/(.+)$/);
    var page = hash.match(/^#\/p\/(\d+)$/);
    window.scrollTo(0, 0);

    if (hash === "#/metrics") {
      renderMetrics();
      return;
    }
    if (detail) {
      renderDetail(decodeURIComponent(detail[1]));
      return;
    }
    renderHome(page ? parseInt(page[1], 10) : 0);
  }

  /* ── mobile menu ───────────────────────────────────────────────────────── */

  var header = document.querySelector(".site-header");
  var toggle = document.getElementById("navtoggle");

  function setMenu(open) {
    if (!header || !toggle) return;
    header.classList.toggle("nav-open", open);
    toggle.setAttribute("aria-expanded", open ? "true" : "false");
  }

  if (toggle) {
    toggle.addEventListener("click", function (e) {
      e.stopPropagation();
      setMenu(header.className.indexOf("nav-open") === -1);
    });
    // Anywhere else, Escape, or following a link all dismiss it — a menu that
    // only closes via its own button is a trap on a phone.
    document.addEventListener("click", function (e) {
      if (header.classList.contains("nav-open") && !header.contains(e.target)) setMenu(false);
    });
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape") setMenu(false);
    });
    var nav = document.getElementById("sitenav");
    if (nav) nav.addEventListener("click", function () { setMenu(false); });
  }

  window.addEventListener("hashchange", function () {
    setMenu(false);
    route();
  });
  route();
})();
