// Sidechain hook — reads live gameplay state off the native HUD and prints it to the console as
// compact JSON lines:  [SIDECHAIN]{"s":"<session>","n":<seq>, ...}
// The desktop engine (sidechain.exe) reads them over -netconport or from console.log (-condebug)
// and turns them into EQ moves on Spotify. This script only reports facts (numbers, texts,
// class edges); all meaning/mapping lives in the engine so tuning never needs a VPK rebuild.
//
// Loaded by the citadel_hud_top_bar.xml override, so $.GetContextPanel() is the CitadelHudTopBar.
(function () {
    "use strict";

    var PREFIX = "[SIDECHAIN]";
    var VERSION = "0.1.2";
    var FAST = 0.033;              // seconds between polls of ammo / health / speed / hits
    var SLOW_EVERY = 4;            // the rest every Nth poll (~130 ms)
    var MISSING_RETRY_MS = 1000;   // throttle FindChildTraverse for panels that aren't there yet
    var ENABLE_SPEEDOMETER = true; // turns on the native movespeed readout so we can read it
    var HIDE_SPEEDOMETER = true;   // ...and makes it invisible (opacity keeps data binding live)

    var ctx = $.GetContextPanel();
    var session = Date.now().toString(36) + Math.floor(Math.random() * 0xffff).toString(36);
    var seq = 0;
    var tick = 0;
    var out = null;  // fields for the line being built this tick
    var last = {};   // last reported value per field
    var cache = {};  // id -> panel
    var missAt = {}; // id -> last failed lookup time

    // ---------------------------------------------------------------- utils

    function valid(p) {
        try { return !!(p && p.IsValid && p.IsValid()); } catch (e) { return false; }
    }
    function has(p, cls) {
        try { return !!(valid(p) && p.BHasClass(cls)); } catch (e) { return false; }
    }
    function text(p) {
        try { return valid(p) && typeof p.text === "string" ? p.text : null; } catch (e) { return null; }
    }
    function num(s) {
        if (s === null || s === undefined) return null;
        var m = String(s).replace(/,/g, "").match(/-?\d+(\.\d+)?/);
        return m ? Number(m[0]) : null;
    }
    function kids(p) {
        try { return valid(p) && p.Children ? p.Children() : []; } catch (e) { return []; }
    }
    function byClass(p, cls) {
        try { return valid(p) ? p.FindChildrenWithClassTraverse(cls) : []; } catch (e) { return []; }
    }
    function hudRoot() {
        var p = ctx;
        for (var i = 0; i < 64; i++) {
            var up = null;
            try { up = p.GetParent(); } catch (e) { up = null; }
            if (!valid(up)) break;
            p = up;
        }
        return p;
    }
    function find(id) {
        var p = cache[id];
        if (valid(p)) return p;
        var now = Date.now();
        if (missAt[id] && now - missAt[id] < MISSING_RETRY_MS) return null;
        try { p = hudRoot().FindChildTraverse(id); } catch (e) { p = null; }
        if (valid(p)) { cache[id] = p; delete missAt[id]; return p; }
        missAt[id] = now;
        return null;
    }
    function ancestorHas(p, cls, depth) {
        for (var i = 0; i < depth && valid(p); i++) {
            if (has(p, cls)) return true;
            try { p = p.GetParent(); } catch (e) { return false; }
        }
        return false;
    }

    // Field changed -> include in this tick's line.
    function field(key, value) {
        if (value === null || value === undefined) return;
        if (last[key] === value) return;
        last[key] = value;
        out[key] = value;
    }
    function event(ev) {
        (out.ev = out.ev || []).push(ev);
    }
    function flush() {
        var any = false;
        for (var k in out) { if (Object.prototype.hasOwnProperty.call(out, k)) { any = true; break; } }
        if (!any) return;
        seq++;
        out.s = session;
        out.n = seq;
        $.Msg(PREFIX + JSON.stringify(out));
    }

    // ---------------------------------------------------------------- fast fields

    var ammoCur = null, ammoMax = null;
    function pollAmmo() {
        if (!valid(ammoCur) || !valid(ammoMax)) {
            var panel = find("ammo_panel");
            if (!panel) return;
            ammoCur = byClass(panel, "weapon_ammo")[0] || null;
            ammoMax = byClass(panel, "weapon_ammo_max")[0] || null;
        }
        field("ammo", num(text(ammoCur)));
        field("ammom", num(text(ammoMax)));
    }

    // hud_health_container: Label.currentHealthLabel {i:r:current_health} /
    // Label.totalHealthLabel "/ {i:r:total_health}" (class-only, no ids). Fallback: #health_bar
    // ProgressBarWithMiddle value/max.
    var hpCur = null, hpTot = null, hpBar = null, hpLookAt = 0;
    function pollHealth() {
        if ((!valid(hpCur) || !valid(hpTot)) && Date.now() - hpLookAt > MISSING_RETRY_MS) {
            hpLookAt = Date.now();
            var root = hudRoot();
            hpCur = byClass(root, "currentHealthLabel")[0] || null;
            hpTot = byClass(root, "totalHealthLabel")[0] || null;
        }
        var cur = num(text(hpCur)), tot = num(text(hpTot));
        if (cur === null || tot === null) {
            if (!valid(hpBar)) hpBar = find("health_bar");
            try {
                if (valid(hpBar)) { cur = Math.round(Number(hpBar.value)); tot = Math.round(Number(hpBar.max)); }
            } catch (e) {}
        }
        if (cur !== null && isFinite(cur)) field("hp", cur);
        if (tot !== null && isFinite(tot) && tot > 0) field("hpm", tot);
    }

    var speedSetup = false;
    function pollSpeed() {
        var bar = find("MovementSpeed");
        var lbl = find("MovementSpeedCurrent");
        if (!speedSetup && (bar || lbl)) {
            speedSetup = true;
            if (HIDE_SPEEDOMETER) {
                // walk up to the CitadelHudMovementSpeed panel and make it invisible
                var p = bar || lbl;
                for (var i = 0; i < 4 && valid(p); i++) {
                    if (p.paneltype === "CitadelHudMovementSpeed") { p.style.opacity = "0"; break; }
                    p = p.GetParent();
                }
            }
        }
        var v = null;
        if (lbl) v = num(text(lbl));
        if (v === null && bar) { try { v = Number(bar.value); } catch (e) { v = null; } }
        if (v !== null && isFinite(v)) field("spd", Math.round(v * 10) / 10);
    }

    var hitWas = false, critWas = false;
    function pollHits() {
        var hm = find("hit_markers");
        if (!hm) return;
        var hit = ancestorHas(hm, "show_hit_marker", 4);
        var crit = ancestorHas(hm, "show_crit_hit_marker", 4);
        if (crit && !critWas) event("crit");
        else if (hit && !hitWas) event("hit");
        hitWas = hit; critWas = crit;
    }

    // damageImpactInfo gets one instance panel per enemy hero you hurt; it gains .killed /
    // .assist / .Critical as things happen. Report class rising edges per instance.
    var impacts = []; // {p, killed, assist}
    function pollImpacts() {
        var box = find("damageImpactInfo");
        if (!box) return;
        var keep = [];
        for (var i = 0; i < impacts.length; i++) if (valid(impacts[i].p)) keep.push(impacts[i]);
        impacts = keep;
        var ch = kids(box);
        for (var j = 0; j < ch.length; j++) {
            var p = ch[j];
            if (!valid(p)) continue;
            var rec = null;
            for (var k = 0; k < impacts.length; k++) if (impacts[k].p === p) { rec = impacts[k]; break; }
            if (!rec) { rec = { p: p, killed: false, assist: false }; impacts.push(rec); event("dmgout"); }
            var killed = has(p, "killed"), assist = has(p, "assist");
            if (killed && !rec.killed) event("kill");
            if (assist && !rec.assist) event("assist");
            rec.killed = killed; rec.assist = assist;
        }
    }

    // ---------------------------------------------------------------- slow fields

    var localPlayer = null;
    function findLocalPlayer() {
        if (valid(localPlayer)) return localPlayer;
        localPlayer = null;
        var ps = byClass(ctx, "LocalPlayer");
        for (var i = 0; i < ps.length; i++) {
            if (ps[i].paneltype === "CitadelHudTopBarPlayer") { localPlayer = ps[i]; break; }
        }
        return localPlayer;
    }

    // Kill-streak popup in our own top-bar slot: its first Label child is the running count.
    var ksLast = null, ksNull = 0;
    function pollKillStreak(player) {
        var node = null;
        try { node = player.FindChildTraverse("KillStreakText"); } catch (e) {}
        var count = null, ch = kids(node);
        for (var i = 0; i < ch.length; i++) {
            if (ch[i].paneltype === "Label") { count = num(text(ch[i])); break; }
        }
        if (count === null) { if (++ksNull >= 3) ksLast = null; return; }
        ksNull = 0;
        if ((ksLast === null && count >= 1) || (ksLast !== null && count > ksLast)) event("ks");
        ksLast = count;
    }

    // All label texts under a panel (html tags stripped), depth-limited.
    function labelTexts(p, out, depth) {
        if (!valid(p) || depth > 8) return out;
        if (p.paneltype === "Label") {
            var t = text(p);
            if (t) {
                t = t.replace(/<[^>]+>/g, "").replace(/\s+/g, " ").trim();
                if (t) out.push(t);
            }
        }
        var ch = kids(p);
        for (var i = 0; i < ch.length; i++) labelTexts(ch[i], out, depth + 1);
        return out;
    }

    // Objective banners ("Enemy Yellow Tier 1" / "DESTROYED"): every label under #TeamMsg.
    // Emitted when the text changes; .IsPositive on an ancestor = good for the local team.
    var teamSig = "";
    function pollTeamMsg() {
        var box = find("TeamMsg");
        if (!box) return;
        var texts = labelTexts(box, [], 0);
        var sig = texts.join("|");
        if (sig === teamSig) return;
        teamSig = sig;
        if (!texts.length) return;
        var pos = ancestorHas(box, "IsPositive", 6);
        event(["txt", "team", texts[0], texts.slice(1).join(" / "), pos ? 1 : 0]);
    }

    // Game announcement toasts. The text is often bound a moment after .ToastVisible lands, so
    // re-read for up to ~2.5 s; if it never fills, emit the panel ids so the toast can be named.
    var toasts = []; // {p, tries, sent}
    function pollToasts() {
        var tm = find("ToastManager");
        if (!tm) return;
        var keep = [];
        for (var i = 0; i < toasts.length; i++) if (valid(toasts[i].p)) keep.push(toasts[i]);
        toasts = keep;
        var ch = kids(tm);
        for (var j = 0; j < ch.length; j++) {
            var t = ch[j];
            if (!has(t, "ToastVisible")) continue;
            var rec = null;
            for (var k = 0; k < toasts.length; k++) if (toasts[k].p === t) { rec = toasts[k]; break; }
            if (!rec) { rec = { p: t, tries: 0, sent: false }; toasts.push(rec); }
            if (rec.sent) continue;
            rec.tries++;
            var texts = labelTexts(t, [], 0);
            if (texts.length) {
                rec.sent = true;
                event(["txt", "toast", texts[0], texts.slice(1).join(" / ")]);
            } else if (rec.tries > 20) {
                rec.sent = true;
                var ids = [];
                var c2 = kids(t);
                for (var m = 0; m < c2.length; m++) ids.push((c2[m].paneltype || "?") + "#" + (c2[m].id || ""));
                event(["txt", "toast", "", "panels: " + ids.join(",")]);
            }
        }
    }

    // Ability bar: report a use when an ability starts cooling down / loses a charge.
    var abil = [];
    function pollAbilities() {
        var rootAb = find("hud_signature");
        if (!rootAb) { abil = []; return; }
        var ps = byClass(rootAb, "ability_container");
        for (var i = 0; i < ps.length; i++) {
            var p = ps[i];
            var names = byClass(p, "ability_name");
            var cur = { p: p, cd: has(p, "cooling_down"), name: names.length ? text(names[0]) || "" : "" };
            var prev = abil[i];
            if (prev && prev.p === p && !prev.cd && cur.cd) event(["ab", i + 1, cur.name]);
            abil[i] = cur;
        }
        abil.length = ps.length;
    }

    function pollSlow() {
        var root = hudRoot();
        var player = findLocalPlayer();
        field("dead", player ? (has(player, "Dead") ? 1 : 0) : null);
        if (player) pollKillStreak(player);
        field("shop", has(ctx, "gShopOpen") ? 1 : 0);
        field("sb", has(ctx, "gScoreboardOpen") ? 1 : 0);
        var tm = find("ToastManager");
        if (tm) field("combat", has(tm.GetParent(), "gInCombat") ? 1 : 0);
        var mb = find("MidbossTimerLabel");
        if (mb) field("mb", text(mb) || "");
        field("live", player ? 1 : 0);
        pollTeamMsg();
        pollToasts();
        pollAbilities();
    }

    // ---------------------------------------------------------------- engine events

    var GAME_EVENTS = ["CitadelGameStateChanged", "CitadelMatchStateChanged", "CitadelConnectedToGame",
        "CitadelResumePlaying", "CitadelHudBossKilled", "CitadelHudMatchStart", "CitadelHudMatchEnd"];
    function hookGameEvents() {
        GAME_EVENTS.forEach(function (name) {
            try {
                $.RegisterForUnhandledEvent(name, function () {
                    var args = [];
                    for (var i = 0; i < arguments.length; i++) {
                        var a = arguments[i];
                        args.push(typeof a === "object" ? String(a) : a);
                    }
                    out = out || {};
                    event(["ge", name].concat(args));
                });
            } catch (e) {}
        });
    }

    // ---------------------------------------------------------------- loop

    function loop() {
        if (!valid(ctx)) return;
        var pending = out && out.ev ? out.ev : null; // events raised by engine callbacks
        out = pending ? { ev: pending } : {};
        try {
            pollAmmo();
            pollHealth();
            pollSpeed();
            pollHits();
            pollImpacts();
            if (tick % SLOW_EVERY === 0) pollSlow();
            if (tick % 150 === 0) field("hb", Math.floor(tick / 150) % 2); // ~5s heartbeat flip
            if (tick % 90 === 45) {
                // keyframe: forget the slow fields so the next slow poll re-sends them (~3 s)
                ["live", "dead", "hpm", "ammom", "shop", "sb", "combat", "mb"].forEach(function (k) { delete last[k]; });
            }
        } catch (e) {
            out.err = String(e);
        }
        flush();
        out = null;
        tick++;
        $.Schedule(FAST, loop);
    }

    if (ENABLE_SPEEDOMETER) {
        try { $.DispatchEvent("CitadelConCommand", "citadel_show_movespeed_on_hud 1"); } catch (e) {}
    }
    hookGameEvents();
    out = { ready: VERSION };
    flush();
    out = null;
    loop();
})();
