"use strict";
function element(id) {
    const node = document.getElementById(id);
    if (!node)
        throw new Error(`Missing element: ${id}`);
    return node;
}
function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}
const shortNames = {
    refactor: "Refactor",
    explain: "Explain",
    tests: "Tests",
    review: "Review",
};
const accessNames = {
    cold: "Cold start",
    ram_hit: "RAM hit",
    disk_restore: "Disk restore",
};
let snapshot = null;
let running = false;
let busy = false;
let timer;
function text(id, value) {
    element(id).textContent = value === undefined ? "" : String(value);
}
function milliseconds(value) {
    return value >= 1000 ? `${(value / 1000).toFixed(2)} s` : `${value} ms`;
}
function bytes(value) {
    return value >= 1024 ? `${(value / 1024).toFixed(1)} KiB` : `${value} bytes`;
}
function controls() {
    element("play").disabled = busy || !snapshot;
    element("step").disabled =
        busy || !snapshot || running || snapshot.step === snapshot.total;
    element("reset").disabled = busy || !snapshot;
    element("budget").disabled = busy || running || !snapshot;
    element("play-icon").src = running ? "/icons/pause.svg" : "/icons/play.svg";
    text("play-label", running
        ? "Pause"
        : snapshot?.step === snapshot?.total
            ? "Replay route"
            : "Run route");
    element("play").title = running ? "Pause route" : "Run scripted route";
}
function tile(session, active) {
    const node = document.createElement("div");
    node.className = `session-tile ${session.spec.id}${active ? " active" : ""}`;
    node.style.flex = `${session.spec.estimated_units} 1 0`;
    node.title = `${session.spec.label}: ${session.spec.estimated_units} modeled units, ${session.spec.rebuild_ms} ms modeled rebuild cost`;
    const name = document.createElement("strong");
    name.textContent = shortNames[session.spec.id];
    const size = document.createElement("span");
    size.textContent = `${session.spec.estimated_units} units`;
    node.append(name, size);
    return node;
}
function policy(view, prefix, outcome) {
    if (!snapshot)
        return;
    const ram = element(`${prefix}-ram`);
    const disk = element(`${prefix}-disk`);
    ram.replaceChildren();
    disk.replaceChildren();
    const resident = view.sessions.filter((session) => session.residency === "ram");
    const checkpointed = view.sessions.filter((session) => session.residency === "disk");
    for (const session of resident)
        ram.append(tile(session, outcome?.session === session.spec.id));
    const remaining = snapshot.budget - view.used_units;
    if (remaining > 0) {
        const empty = document.createElement("div");
        empty.className = "free-space";
        empty.style.flex = `${remaining} 1 0`;
        empty.textContent = `${remaining} free`;
        ram.append(empty);
    }
    for (const session of checkpointed) {
        const node = document.createElement("span");
        node.className = `disk-item ${session.spec.id}`;
        node.textContent = `${shortNames[session.spec.id]} / ${bytes(session.checkpoint_bytes)}`;
        node.title = `${session.spec.label}: ${session.turns} recorded turns`;
        disk.append(node);
    }
    if (!checkpointed.length) {
        const empty = document.createElement("span");
        empty.className = "disk-empty";
        empty.textContent = "No evicted sessions";
        disk.append(empty);
    }
    text(`${prefix}-capacity`, `${view.used_units} / ${snapshot.budget} units`);
    text(`${prefix}-disk-count`, `${checkpointed.length} on disk`);
    text(`${prefix}-hits`, view.stats.ram_hits);
    text(`${prefix}-restores`, view.stats.disk_restores);
    text(`${prefix}-time`, milliseconds(view.stats.modeled_ms));
    const decision = element(`${prefix}-decision`);
    decision.className = `decision ${outcome?.access === "ram_hit" ? "hit" : outcome?.access === "disk_restore" ? "restore" : ""}`;
    decision.textContent = outcome
        ? `${accessNames[outcome.access]} / ${outcome.modeled_ms} modeled ms${outcome.evicted.length ? ` / checkpointed ${outcome.evicted.map((id) => shortNames[id]).join(", ")}` : ""}`
        : "Awaiting first request";
}
function render(state) {
    const changed = snapshot?.step !== state.step;
    snapshot = state;
    element("budget").value = String(state.budget);
    text("budget-value", `${state.budget} units`);
    element("route-count").replaceChildren(document.createTextNode(`${String(state.step).padStart(2, "0")} `));
    const total = document.createElement("span");
    total.textContent = `/ ${state.total}`;
    element("route-count").append(total);
    const delta = state.policies[0].stats.modeled_ms - state.policies[1].stats.modeled_ms;
    element("time-delta").replaceChildren(document.createTextNode(`${delta} `));
    const unit = document.createElement("span");
    unit.textContent = "ms";
    element("time-delta").append(unit);
    element("checkpoint-count").replaceChildren(document.createTextNode(`${state.checkpoint_files} `));
    const files = document.createElement("span");
    files.textContent = "files";
    element("checkpoint-count").append(files);
    text("checkpoint-bytes", bytes(state.checkpoint_bytes));
    element("route-track").replaceChildren();
    for (let index = 0; index < state.total; index += 1) {
        const marker = document.createElement("span");
        if (index < state.step)
            marker.className = "done";
        element("route-track").append(marker);
    }
    text("request-number", String(state.step).padStart(2, "0"));
    text("request-kind", state.current
        ? `SESSION / ${state.current.task.spec.id.toUpperCase()}`
        : "DISPATCH QUEUE");
    text("request-title", state.current?.task.spec.label || "Route ready");
    text("request-prompt", state.current?.task.prompt || "No requests dispatched.");
    text("response", state.current?.task.response || "No response yet.");
    text("request-state", state.step === state.total
        ? "COMPLETE"
        : running
            ? "RUNNING"
            : state.step
                ? "PAUSED"
                : "IDLE");
    text("runtime-state", state.step === state.total
        ? "Route complete"
        : running
            ? "Dispatching"
            : "Runtime ready");
    policy(state.policies[0], "lru", state.current?.outcomes[0]);
    policy(state.policies[1], "cost", state.current?.outcomes[1]);
    const log = element("log-body");
    log.replaceChildren();
    for (const turn of [...state.history].slice(-5).reverse()) {
        const row = document.createElement("tr");
        const values = [
            String(turn.index).padStart(2, "0"),
            shortNames[turn.task.spec.id],
            accessNames[turn.outcomes[0].access],
            accessNames[turn.outcomes[1].access],
            `${turn.outcomes[0].modeled_ms - turn.outcomes[1].modeled_ms} ms`,
        ];
        values.forEach((value, index) => {
            const cell = document.createElement("td");
            cell.textContent = value;
            if (index === 2 || index === 3)
                cell.className =
                    turn.outcomes[index - 2].access === "ram_hit"
                        ? "access-hit"
                        : turn.outcomes[index - 2].access === "disk_restore"
                            ? "access-restore"
                            : "";
            row.append(cell);
        });
        log.append(row);
    }
    if (!state.history.length) {
        const row = document.createElement("tr");
        const cell = document.createElement("td");
        cell.colSpan = 5;
        cell.className = "empty-log";
        cell.textContent = "No requests yet";
        row.append(cell);
        log.append(row);
    }
    if (changed) {
        const request = document.querySelector(".current-request");
        if (request) {
            request.classList.remove("changed");
            requestAnimationFrame(() => request.classList.add("changed"));
        }
    }
    controls();
}
async function call(path, body) {
    busy = true;
    controls();
    element("error").hidden = true;
    try {
        const response = await fetch(path, {
            method: body === undefined && path === "/api/state" ? "GET" : "POST",
            headers: body === undefined ? {} : { "Content-Type": "application/json" },
            body: body === undefined ? undefined : JSON.stringify(body),
        });
        const state = await response.json();
        if (!response.ok)
            throw new Error(state.error || `Request failed (${response.status})`);
        render(state);
        return true;
    }
    catch (error) {
        running = false;
        clearTimeout(timer);
        text("error", errorMessage(error));
        element("error").hidden = false;
        text("runtime-state", "Runtime unavailable");
        return false;
    }
    finally {
        busy = false;
        controls();
    }
}
async function advance() {
    if (busy || !snapshot || snapshot.step === snapshot.total)
        return;
    const success = await call("/api/step");
    if (!success)
        return;
    if (snapshot.step === snapshot.total) {
        running = false;
        controls();
    }
    else if (running)
        timer = setTimeout(advance, 1100);
}
element("play").addEventListener("click", async () => {
    if (!snapshot)
        return;
    if (running) {
        running = false;
        clearTimeout(timer);
        render(snapshot);
        return;
    }
    if (snapshot.step === snapshot.total) {
        if (!(await call("/api/reset", { budget: snapshot.budget })))
            return;
    }
    running = true;
    render(snapshot);
    await advance();
});
element("step").addEventListener("click", advance);
element("reset").addEventListener("click", async () => {
    running = false;
    clearTimeout(timer);
    await call("/api/reset", { budget: Number(element("budget").value) });
});
element("budget").addEventListener("input", () => text("budget-value", `${element("budget").value} units`));
element("budget").addEventListener("change", async () => {
    await call("/api/reset", { budget: Number(element("budget").value) });
});
call("/api/state");
let liveConfigured = false;
let liveBusy = false;
function measuredMs(value) {
    return `${value.toFixed(2)} ms`;
}
function renderLive(report) {
    element("live-metrics").hidden = false;
    element("live-verdict").hidden = false;
    element("live-report").hidden = false;
    text("live-status", `${report.identity.engine_build} / ${report.prompt_tokens} prompt tokens / ${report.identity.context_tokens}-token slot / verified`);
    text("live-cold", measuredMs(report.cold_prefill.elapsed_ms));
    text("live-cold-tokens", `${report.cold_prefill.engine.timings.prompt_n} tokens evaluated`);
    text("live-bytes", bytes(report.save.engine.bytes));
    text("live-save", `save + validation: ${measuredMs(report.save.elapsed_ms)}`);
    text("live-restore", measuredMs(report.restore.elapsed_ms));
    text("live-restored-tokens", `${report.restore.engine.n_restored} state tokens restored`);
    text("live-warm", measuredMs(report.reused_prefill.elapsed_ms));
    text("live-cached-tokens", `${report.reused_prefill.engine.timings.cache_n} reused / ${report.reused_prefill.engine.timings.prompt_n} evaluated`);
    const restoreCost = report.restore.elapsed_ms + report.reused_prefill.elapsed_ms;
    const delta = report.cold_prefill.elapsed_ms - restoreCost;
    text("live-verdict", `Restore + resumed prefill: ${measuredMs(restoreCost)} / ${measuredMs(Math.abs(delta))} ${delta >= 0 ? "faster" : "slower"} than fresh prefill in this observation. Save cost is additional.`);
    if (report.continued_prefill) {
        element("live-verdict").append(document.createTextNode(` Continuation: ${report.continued_prefill.engine.timings.cache_n} prefix tokens reused / ${report.continued_prefill.engine.timings.prompt_n} new tokens evaluated.`));
    }
    text("live-json", JSON.stringify(report, null, 2));
}
async function initializeLive() {
    try {
        const response = await fetch("/api/live/state");
        if (!response.ok)
            throw new Error(`Live configuration unavailable (${response.status})`);
        const state = await response.json();
        liveConfigured = state.configured;
        element("live-run").disabled = !liveConfigured || state.busy;
        text("live-status", liveConfigured
            ? "Dedicated engine configured / ready for a measured checkpoint round trip"
            : "Not configured / simulator only");
        if (state.report)
            renderLive(state.report);
    }
    catch (error) {
        text("live-status", errorMessage(error));
    }
}
element("live-run").addEventListener("click", async () => {
    if (!liveConfigured || liveBusy)
        return;
    liveBusy = true;
    element("live-run").disabled = true;
    text("live-run-label", "Checking KV state");
    text("live-status", "Running actual prefill, checkpoint, restore, and prefix-reuse verification");
    try {
        const response = await fetch("/api/live/probe", { method: "POST" });
        const report = await response.json();
        if (!response.ok)
            throw new Error(report.error || `Checkpoint test failed (${response.status})`);
        renderLive(report);
    }
    catch (error) {
        text("live-status", `Live test failed: ${errorMessage(error)}`);
    }
    finally {
        liveBusy = false;
        element("live-run").disabled = !liveConfigured;
        text("live-run-label", "Run checkpoint test");
    }
});
initializeLive();
const codingView = new URLSearchParams(location.search).get("view") === "coding";
const repositoryView = new URLSearchParams(location.search).get("view") === "repository";
document.querySelectorAll(".lab-tabs a").forEach((link, index) => {
    if (index === (repositoryView ? 2 : codingView ? 1 : 0))
        link.setAttribute("aria-current", "page");
});
if (codingView) {
    element("coding-frame").hidden = false;
    for (const selector of [
        "#demo-frame",
        ".live-lab",
        ".dispatch-log",
        ".transcript",
    ]) {
        const section = document.querySelector(selector);
        if (section)
            section.hidden = true;
    }
}
let codingConfigured = false;
let codingBusy = false;
let codingStage = "ready";
const codingCommands = {
    ready: "Generate refactor",
    answered: "Save + clear slot",
    checkpointed: "Restore + follow-up",
    complete: "Complete",
    failed: "Failed",
};
function codingControls() {
    element("coding-next").disabled =
        !codingConfigured ||
            codingBusy ||
            ["complete", "failed"].includes(codingStage);
    element("coding-reset").disabled = !codingConfigured || codingBusy;
    text("coding-next-label", codingBusy ? "Working locally" : codingCommands[codingStage]);
}
function renderCoding(report) {
    codingStage = report?.stage || "ready";
    text("coding-original", report?.original ||
        "fn invoice_total(unit_price: u64, quantity: u64, discount_percent: u64) -> u64 {\n    let gross = unit_price * quantity;\n    let discount = gross * discount_percent / 100;\n    gross - discount\n}");
    text("coding-answer", report?.first?.engine.content || "Awaiting local model");
    text("coding-followup", report?.followup?.engine.content || "Awaiting restored conversation");
    text("coding-reference", report?.followup_reference ||
        "Test oracle: (101, 3, 10) -> Some(273); (u64::MAX, 1, 0) -> Some(u64::MAX); (u64::MAX, 1, 50) -> None. Code tests do not validate the model's explanation.");
    text("coding-status", {
        ready: codingConfigured
            ? "Dedicated coding engine configured / ready"
            : "No dedicated coding engine configured",
        answered: "Model response received / conversation resident",
        checkpointed: "Binary KV saved / engine slot cleared",
        complete: "Follow-up generated from restored KV / engine slot cleared",
        failed: "Experiment failed / reset to retry",
    }[codingStage]);
    const count = {
        ready: 0,
        answered: 1,
        checkpointed: 2,
        complete: 3,
        failed: 0,
    }[codingStage];
    document
        .querySelectorAll(".coding-timeline li")
        .forEach((node, index) => node.classList.toggle("done", index < count));
    const validation = report?.validation;
    text("coding-validation", validation
        ? validation.passed
            ? `Actual source allowlisted / ${validation.cases} invoice cases passed`
            : validation.accepted
                ? "Allowed sample / compiler or tests failed"
                : "Output rejected / no generated code executed"
        : "Invoice tests / pending");
    element("coding-validation").classList.toggle("passed", Boolean(validation?.passed));
    text("coding-context", report?.identity
        ? `${report.identity.context_tokens}-token slot / ${report.output_limit}-token output cap`
        : "256-token output cap");
    text("coding-first-time", report?.first ? milliseconds(Math.round(report.first.elapsed_ms)) : "--");
    text("coding-first-tokens", report?.first
        ? `${report.first.engine.timings.predicted_n} output tokens / ${report.first.engine.stop_type}`
        : "--");
    text("coding-bytes", report?.save ? bytes(report.save.engine.bytes) : "--");
    text("coding-save-time", report?.save
        ? `save + integrity: ${measuredMs(report.save.elapsed_ms)}`
        : "--");
    text("coding-restore-time", report?.restore ? measuredMs(report.restore.elapsed_ms) : "--");
    text("coding-restored", report?.restore
        ? `${report.restore.engine.n_restored} KV tokens restored`
        : "--");
    text("coding-reused", report?.followup
        ? `${report.followup.engine.timings.cache_n} reused`
        : "--");
    text("coding-new-tokens", report?.followup
        ? `${report.followup.engine.timings.prompt_n} new input tokens evaluated`
        : "--");
    text("coding-detail", report?.followup
        ? `Follow-up: ${milliseconds(Math.round(report.followup.elapsed_ms))} / canonical prefix: ${report.canonical_prefix_tokens} tokens / ${report.boundary_tokens_omitted} boundary tokens excluded / no speedup claim`
        : "Resident model / dedicated slot / no speedup claim");
    element("coding-report").hidden = !codingView || !report;
    text("coding-json", JSON.stringify(report, null, 2));
    codingControls();
}
async function initializeCoding() {
    if (!codingView)
        return;
    try {
        const response = await fetch("/api/coding/state");
        if (!response.ok)
            throw new Error(`Coding configuration unavailable (${response.status})`);
        const state = await response.json();
        codingConfigured = state.configured;
        codingBusy = state.busy;
        renderCoding(state.report);
        if (state.busy) {
            text("coding-status", "A live experiment is running");
            setTimeout(initializeCoding, 1000);
        }
    }
    catch (error) {
        text("coding-status", errorMessage(error));
    }
}
async function codingOperation(path) {
    if (codingBusy)
        return;
    codingBusy = true;
    codingControls();
    const progress = {
        ready: "Generating a real local coding response",
        answered: "Prefilling canonical prefix, saving KV, and clearing slot",
        checkpointed: "Verifying checkpoint, restoring KV, and generating follow-up",
    };
    text("coding-status", path.endsWith("reset") ? "Clearing dedicated slot" : progress[codingStage]);
    try {
        const response = await fetch(path, { method: "POST" });
        const report = await response.json();
        if (!response.ok)
            throw new Error(report.error || `Coding experiment failed (${response.status})`);
        renderCoding(path.endsWith("reset") ? null : report);
    }
    catch (error) {
        codingStage = "failed";
        text("coding-status", `Failed: ${errorMessage(error)}`);
    }
    finally {
        codingBusy = false;
        codingControls();
    }
}
element("coding-next").addEventListener("click", () => codingOperation("/api/coding/step"));
element("coding-reset").addEventListener("click", () => codingOperation("/api/coding/reset"));
initializeCoding();
if (repositoryView) {
    element("repository-frame").hidden = false;
    for (const selector of [
        "#demo-frame",
        ".live-lab",
        ".dispatch-log",
        ".transcript",
    ]) {
        const section = document.querySelector(selector);
        if (section)
            section.hidden = true;
    }
}
let repositoryConfigured = false;
let repositoryBusy = false;
let repositoryStage = "ready";
const repositoryCommands = {
    ready: "Read + propose feature",
    proposed: "Review, test + park",
    parked: "Run helper request",
    helper_done: "Restore, revise + test",
    complete: "Complete",
    failed: "Failed",
};
function repositoryControls() {
    element("repository-next").disabled =
        !repositoryConfigured ||
            repositoryBusy ||
            ["complete", "failed"].includes(repositoryStage);
    element("repository-reset").disabled =
        !repositoryConfigured || repositoryBusy;
    text("repository-command", repositoryBusy ? "Working locally" : repositoryCommands[repositoryStage]);
}
function renderRepository(report) {
    repositoryStage = report?.stage || "ready";
    text("repository-status", {
        ready: repositoryConfigured
            ? "Dedicated engine ready / read src/main.rs to propose report download policy"
            : "No dedicated engine configured",
        proposed: "Real source read / model proposal ready for strict review",
        parked: "Temporary-project checks recorded / KV saved / engine slot cleared",
        helper_done: "Unrelated helper completed in the same slot / repository conversation remains on disk",
        complete: "Repository KV restored / revised policy tested in disposable snapshot",
        failed: "Repository experiment failed / reset to retry",
    }[repositoryStage]);
    text("repository-source", report?.context.excerpt || "Awaiting allowlisted source read");
    text("repository-proposal", report?.revision?.engine.content ||
        report?.proposal?.engine.content ||
        "Awaiting real feature proposal");
    const check = report?.final_check || report?.initial_check;
    text("repository-tests", check
        ? check.passed
            ? "3 API tests passed / 8 status combinations / offline snapshot"
            : check.accepted
                ? "Reviewed policy / project checks failed"
                : "Proposal rejected / no generated code executed"
        : "Disposable project / not tested");
    text("repository-helper", report?.helper?.engine.content || "Awaiting parked conversation");
    text("repository-hash", report?.context.sha256.slice(0, 12) || "--");
    text("repository-bytes", report?.checkpoint ? bytes(report.checkpoint.engine.bytes) : "--");
    text("repository-save", report?.checkpoint
        ? `save + integrity: ${measuredMs(report.checkpoint.elapsed_ms)}`
        : "--");
    text("repository-restore", report?.restore ? measuredMs(report.restore.elapsed_ms) : "--");
    text("repository-restored", report?.restore
        ? `${report.restore.engine.n_restored} state tokens restored`
        : "--");
    text("repository-reused", report?.revision
        ? `${report.revision.engine.timings.cache_n} reused`
        : "--");
    text("repository-new", report?.revision
        ? `${report.revision.engine.timings.prompt_n} feedback tokens evaluated`
        : "--");
    text("repository-result", report?.revision
        ? `Restored model revision: ${milliseconds(Math.round(report.revision.elapsed_ms))} / proposal only / neither repository modified`
        : "Proposal only / no changes applied to either repository");
    const count = {
        ready: 0,
        proposed: 1,
        parked: 2,
        helper_done: 3,
        complete: 4,
        failed: 0,
    }[repositoryStage];
    document
        .querySelectorAll(".repository-timeline li")
        .forEach((node, index) => node.classList.toggle("done", index < count));
    element("repository-details").hidden = !repositoryView || !report;
    text("repository-json", JSON.stringify(report, null, 2));
    repositoryControls();
}
async function initializeRepository() {
    if (!repositoryView)
        return;
    try {
        const response = await fetch("/api/repository/state");
        if (!response.ok)
            throw new Error(`Repository configuration failed (${response.status})`);
        const state = await response.json();
        repositoryConfigured = state.configured;
        repositoryBusy = state.busy;
        renderRepository(state.report);
        if (state.busy)
            setTimeout(initializeRepository, 1000);
    }
    catch (error) {
        text("repository-status", errorMessage(error));
    }
}
async function repositoryOperation(reset) {
    if (repositoryBusy)
        return;
    repositoryBusy = true;
    repositoryControls();
    const progress = {
        ready: "Reading actual source and generating feature policy",
        proposed: "Reviewing pure AST / running offline tests in temporary project / saving KV",
        parked: "Generating unrelated helper answer in the same engine slot",
        helper_done: "Restoring repository context / revising against test feedback / rerunning checks",
    };
    text("repository-status", reset
        ? "Resetting dedicated engine experiments"
        : progress[repositoryStage]);
    try {
        if (reset) {
            const cleared = await fetch("/api/coding/reset", { method: "POST" });
            if (!cleared.ok)
                throw new Error("Cannot reset active coding experiment");
        }
        const response = await fetch(`/api/repository/${reset ? "reset" : "step"}`, { method: "POST" });
        const report = await response.json();
        if (!response.ok)
            throw new Error(report.error || `Repository task failed (${response.status})`);
        renderRepository(reset ? null : report);
    }
    catch (error) {
        repositoryStage = "failed";
        text("repository-status", `Failed: ${errorMessage(error)}`);
    }
    finally {
        repositoryBusy = false;
        repositoryControls();
    }
}
element("repository-next").addEventListener("click", () => repositoryOperation(false));
element("repository-reset").addEventListener("click", () => repositoryOperation(true));
initializeRepository();
export {};
