// FreeDAWm service remote: served by the booth, plain JS, no install.
// Pairing (4-digit code from the booth's screen) → a token kept in this browser; the status
// arrives as server-sent events (/api/events); the buttons POST /api/action.
"use strict";
(function () {
  var KEY = "freedawm.remote.token", NAME = "freedawm.remote.name";
  var $ = function (id) { return document.getElementById(id); };
  var store = {
    get: function (k) { try { return localStorage.getItem(k) || ""; } catch (e) { return ""; } },
    set: function (k, v) { try { localStorage.setItem(k, v); } catch (e) {} },
    del: function (k) { try { localStorage.removeItem(k); } catch (e) {} }
  };
  var token = store.get(KEY), myName = store.get(NAME);
  var status = null, source = null, lastEvent = 0, request = "";

  function defaultName() {
    var ua = navigator.userAgent || "";
    if (/iPhone|Android.*Mobile/.test(ua)) return "Phone";
    if (/iPad|Android/.test(ua)) return "Tablet";
    return "Stage PC";
  }

  function api(method, path, body) {
    return fetch(path, {
      method: method,
      headers: { "Content-Type": "application/json", "X-Token": token },
      body: body ? JSON.stringify(body) : undefined,
      cache: "no-store"
    }).then(function (r) {
      return r.json().catch(function () { return {}; }).then(function (j) { j._code = r.status; return j; });
    });
  }

  // ---- pairing ----
  function showPair(msg) {
    if (source) { source.close(); source = null; }
    $("remote").hidden = true;
    $("pair").hidden = false;
    $("pair-name").value = myName || defaultName();
    $("pair-msg").textContent = msg || "";
  }
  $("pair-ask").onclick = function () {
    myName = ($("pair-name").value || defaultName()).trim();
    store.set(NAME, myName);
    api("POST", "/api/pair/request", { name: myName }).then(function (r) {
      if (r._code !== 200) { $("pair-msg").textContent = r.error || "The booth said no."; return; }
      request = r.request;
      $("pair-code-row").hidden = false;
      $("pair-code").value = "";
      $("pair-code").focus();
      $("pair-msg").textContent = "Type the 4 digits " + (r.pc || "the booth") + " shows.";
    }).catch(function () { $("pair-msg").textContent = "Cannot reach the booth."; });
  };
  $("pair-ok").onclick = function () {
    api("POST", "/api/pair/confirm", { request: request, code: $("pair-code").value.trim() }).then(function (r) {
      if (r._code === 200 && r.token) {
        token = r.token;
        store.set(KEY, token);
        $("pair-code-row").hidden = true;
        start();
      } else {
        $("pair-msg").textContent = r.error || "Wrong code.";
        if (!r.left) { $("pair-code-row").hidden = true; }
      }
    });
  };

  // ---- status ----
  function start() {
    $("pair").hidden = true;
    $("remote").hidden = false;
    if (source) source.close();
    source = new EventSource("/api/events?token=" + encodeURIComponent(token));
    source.onmessage = function (e) {
      lastEvent = Date.now();
      try { status = JSON.parse(e.data); } catch (x) { return; }
      render();
    };
    source.onerror = function () {
      // A forgotten token: back to pairing. Otherwise EventSource reconnects by itself.
      api("GET", "/api/status").then(function (r) { if (r._code === 401) { store.del(KEY); token = ""; showPair("This browser was forgotten by the booth: pair again."); } })
        .catch(function () {});
    };
  }

  function hms(s) {
    s = Math.max(0, Math.floor(s || 0));
    var h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, x = s % 60;
    return (h < 10 ? "0" : "") + h + ":" + (m < 10 ? "0" : "") + m + ":" + (x < 10 ? "0" : "") + x;
  }
  function ms(s) {
    s = Math.max(0, Math.floor(s || 0));
    var h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, x = s % 60;
    return (h ? h + ":" + (m < 10 ? "0" : "") : "") + m + ":" + (x < 10 ? "0" : "") + x;
  }
  function gb(b) {
    if (b == null) return "";
    var g = b / 1e9;
    return (g >= 10 ? g.toFixed(0) : g.toFixed(1)) + " GB free";
  }
  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function pcCard(name, detail, state, cls) {
    var c = el("div", "card pc"), l = el("div");
    l.appendChild(el("b", "", name));
    l.appendChild(el("br"));
    l.appendChild(el("small", "", detail));
    c.appendChild(l);
    c.appendChild(el("span", "st " + (cls || ""), state));
    return c;
  }

  function render() {
    var s = status;
    if (!s) return;
    var rec = s.transport === "recording", playing = s.transport === "playing";
    $("session").textContent = s.session || "Untitled";
    var missing = (s.remotes || []).filter(function (r) { return r.state === "not reachable"; }).length;
    var connected = 1 + (s.remotes || []).length - missing + (s.clients || []).length;
    $("rig-line").textContent = (s.pc || "Booth") + " · " + (missing ? missing + " PC" + (missing > 1 ? "s" : "") + " missing" : connected + " connected");
    $("rec-badge").hidden = !rec;
    $("warning").hidden = !s.warning;
    $("warning").textContent = s.warning ? "⚠ " + s.warning : "";
    $("clock").textContent = rec ? hms(s.record_elapsed) : hms(s.position);
    $("sub").textContent = "Bar " + (s.bar || 1) + " · Playhead " + ms(s.position);
    $("b-record").classList.toggle("on", rec);
    $("b-play").classList.toggle("on", playing);
    $("b-stop").classList.toggle("guard", rec);
    var tracks = s.tracks || [];
    var allArmed = tracks.length > 0 && tracks.every(function (t) { return t.armed; });
    $("b-arm").textContent = allArmed ? "○ Disarm All" : "⦿ Arm All";
    $("b-arm").dataset.value = allArmed ? "false" : "true";

    var rig = $("rig");
    rig.textContent = "";
    var left = s.record_hours_left != null ? " · " + Math.floor(s.record_hours_left) + " h left" : "";
    rig.appendChild(pcCard(s.pc || "Booth", gb(s.disk_free) + left, rec ? "● recording" : (playing ? "▶ playing" : "ready"), rec ? "r" : ""));
    (s.remotes || []).forEach(function (r) {
      var detail = r.state === "not reachable" && r.last_seen != null ? "last seen " + ms(r.last_seen) + " ago" : [r.detail, r.disk_free != null ? gb(r.disk_free) : ""].filter(Boolean).join(" · ");
      var cls = r.warning ? "w" : (r.state === "recording" ? "r" : (r.state === "take received" ? "g" : ""));
      var label = (r.warning ? "⚠ " : (r.state === "recording" ? "● " : "")) + r.state;
      rig.appendChild(pcCard(r.name, detail, label, cls));
    });
    var seen = {};
    (s.clients || []).forEach(function (c) {
      if (seen[c]) return;
      seen[c] = 1;
      rig.appendChild(pcCard(c, c === myName ? "this remote" : "remote", "connected", ""));
    });

    var list = $("tracks");
    list.textContent = "";
    tracks.forEach(function (t) {
      var row = el("div", "trk"), arm = el("button", "arm" + (t.armed ? " on" : ""));
      arm.setAttribute("aria-label", (t.armed ? "Disarm " : "Arm ") + t.name);
      arm.onclick = function () { act({ action: "arm", track: t.id, value: !t.armed }); };
      var name = el("div", "", t.name);
      if (t.remote) name.appendChild(el("span", "tag", "from " + t.remote));
      var meter = el("div", "meter"), bar = el("div");
      bar.style.width = Math.round(Math.max(0, Math.min(1, t.level || 0)) * 100) + "%";
      meter.appendChild(bar);
      row.appendChild(arm); row.appendChild(name); row.appendChild(meter);
      list.appendChild(row);
    });
    if (!tracks.length) list.appendChild(el("div", "dim", "No tracks."));

    var mk = $("markers");
    mk.textContent = "";
    (s.markers || []).slice(-8).reverse().forEach(function (m) {
      var row = el("div", "mk"), name = el("span", "", m.number + ". " + m.name + "  ");
      name.appendChild(el("small", "dim", ms(m.at)));
      var rn = el("button", "", "Rename");
      rn.onclick = function () { rename(m.id, m.name); };
      row.appendChild(name); row.appendChild(rn);
      mk.appendChild(row);
    });
    if (!(s.markers || []).length) mk.appendChild(el("div", "dim", "None yet."));
  }

  // ---- buttons ----
  var toastTimer = 0;
  function toast(text, err, extra) {
    var t = $("toast");
    t.textContent = "";
    t.className = "toast" + (err ? " err" : "");
    t.appendChild(el("span", "", text));
    if (extra) t.appendChild(extra);
    t.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () { t.hidden = true; }, 5000);
  }
  function act(body) {
    return api("POST", "/api/action", body).then(function (r) {
      if (r._code === 401) { showPair("Pair again."); return r; }
      if (r._code !== 200) toast(r.error || "The booth said no.", true);
      return r;
    }).catch(function () { toast("Cannot reach the booth.", true); });
  }
  function rename(id, old) {
    var name = window.prompt("Marker name", old || "");
    if (name && name.trim()) act({ action: "rename_marker", id: id, name: name.trim() });
  }
  $("b-record").onclick = function () { act({ action: "record" }); };
  $("b-play").onclick = function () { act({ action: "play" }); };
  $("b-arm").onclick = function () { act({ action: "arm_all", value: this.dataset.value !== "false" }); };
  $("b-marker").onclick = function () {
    act({ action: "marker" }).then(function (r) {
      if (r && r._code === 200 && r.result) {
        var b = el("button", "", "Rename");
        var id = r.result.id;
        b.onclick = function () { rename(id, ""); };
        toast("Marker added", false, b);
      }
    });
  };

  // Stop: a tap stops playback; while recording it must be held ~1 s (the bar fills).
  var stopBtn = $("b-stop"), holdTimer = 0, held = false;
  function holdStart(e) {
    e.preventDefault();
    held = false;
    if (!(status && status.transport === "recording")) return;
    stopBtn.classList.add("holding");
    holdTimer = setTimeout(function () {
      held = true;
      stopBtn.classList.remove("holding");
      if (navigator.vibrate) navigator.vibrate(40);
      act({ action: "stop", held: true });
    }, 1000);
  }
  function holdEnd(e) {
    e.preventDefault();
    var recording = status && status.transport === "recording";
    clearTimeout(holdTimer);
    stopBtn.classList.remove("holding");
    if (held) { held = false; return; }
    if (recording) { toast("Recording: hold Stop to stop it.", true); return; }
    act({ action: "stop" });
  }
  function holdCancel() { clearTimeout(holdTimer); stopBtn.classList.remove("holding"); }
  stopBtn.addEventListener("pointerdown", holdStart);
  stopBtn.addEventListener("pointerup", holdEnd);
  stopBtn.addEventListener("pointerleave", holdCancel);
  stopBtn.addEventListener("pointercancel", holdCancel);
  stopBtn.addEventListener("contextmenu", function (e) { e.preventDefault(); });

  // A silent booth: say so.
  setInterval(function () {
    if (!$("remote").hidden && lastEvent && Date.now() - lastEvent > 15000) $("rig-line").textContent = "Booth not answering… reconnecting";
  }, 3000);

  if (token) start(); else showPair("");
})();
