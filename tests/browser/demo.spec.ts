import { test, expect, type Page } from "@playwright/test";
import type {
  CodingReport,
  RepositoryReport,
  Snapshot,
  ProbeReport,
} from "../../web/api";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
type Completed<T> = { [Key in keyof T]: NonNullable<T[Key]> };

test("recorded coding and repository reports render without inference", async ({
  page,
}) => {
  for (const view of ["coding", "repository"] as const) {
    const report = JSON.parse(
      await readFile(path.resolve(`assets/${view}-report.json`), "utf8"),
    ) as Completed<CodingReport> | Completed<RepositoryReport>;
    await page.route(`**/api/${view}/state`, (route) =>
      route.fulfill({ json: { configured: true, busy: false, report } }),
    );
    await page.goto(`/?view=${view}`);
    await expect(page.locator(`#${view}-next`)).toBeDisabled();
    const content =
      "first" in report
        ? report.first.engine.content
        : report.revision.engine.content;
    await expect(
      page.locator(
        view === "coding" ? "#coding-answer" : "#repository-proposal",
      ),
    ).toHaveText(content);
    await expect(
      page.locator(view === "coding" ? "#coding-reused" : "#repository-reused"),
    ).toContainText("reused");
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth > innerWidth,
      ),
    ).toBe(false);
  }
});

test("opt-in repository feature with checkpointed feedback and GIF frames", async ({
  page,
}, info) => {
  test.skip(
    process.env.CACHE_CAB_REPOSITORY_TEST !== "1",
    "Requires the dedicated Qwen engine and offline Cargo cache",
  );
  test.setTimeout(480000);
  await page.goto("/?view=repository");
  await expect(page.locator("#repository-reset")).toBeEnabled();
  await page.locator("#repository-reset").click();
  await expect(page.locator("#repository-next")).toBeEnabled();
  const frames = path.resolve("captures/repository");
  await mkdir(frames, { recursive: true });
  const region = page.locator("#repository-frame");
  const dimensions = await region.boundingBox();
  if (!dimensions) throw new Error("Repository frame is not visible");
  const capture = async (index: number) => {
    const box = await region.boundingBox();
    if (!box) throw new Error("Repository frame is not visible");
    expect({ width: box.width, height: box.height }).toEqual({
      width: dimensions.width,
      height: dimensions.height,
    });
    if (info.project.name === "desktop")
      await region.screenshot({
        path: path.join(frames, `frame-${index}.png`),
        animations: "disabled",
      });
  };
  await capture(0);
  const trace: RepositoryReport[] = [];
  for (const [index, stage] of [
    [1, "proposed"],
    [2, "parked"],
    [3, "helper_done"],
    [4, "complete"],
  ]) {
    const received = page.waitForResponse(
      (response) => response.url().endsWith("/api/repository/step"),
      { timeout: 240000 },
    );
    await page.locator("#repository-next").click();
    const response = await received;
    const report: RepositoryReport = await response.json();
    expect(response.ok(), JSON.stringify(report)).toBe(true);
    expect(report.stage).toBe(stage);
    trace.push(report);
    await expect(page.locator("#repository-command")).not.toHaveText(
      "Working locally",
    );
    await capture(Number(index));
  }
  const report = trace.at(-1) as Completed<RepositoryReport>;
  expect(report.context.path).toBe("src/main.rs");
  expect(report.context.sha256).toMatch(/^[a-f0-9]{64}$/);
  expect(report.proposal.engine.content).toContain("report_status");
  expect(report.initial_check.passed, report.initial_check.output).toBe(true);
  expect(report.final_check.passed, report.final_check.output).toBe(true);
  expect(report.final_check.output).toContain("3 passed");
  expect(report.checkpoint.engine.bytes).toBe(report.restore.engine.n_read);
  expect(report.helper.engine.content.length).toBeGreaterThan(0);
  expect(report.revision.engine.timings.cache_n).toBeGreaterThan(0);
  expect(report.revision.engine.timings.prompt_n).toBeGreaterThan(0);
  await expect(page.locator("#repository-proposal")).toHaveText(
    report.revision.engine.content,
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth,
    ),
  ).toBe(false);
  await region.screenshot({
    path: path.resolve(`assets/repository-${info.project.name}.png`),
    animations: "disabled",
  });
  if (info.project.name === "desktop") {
    await writeFile(
      path.join(frames, "trace.json"),
      JSON.stringify(trace, null, 2),
    );
    await writeFile(
      path.resolve("assets/repository-report.json"),
      JSON.stringify(report, null, 2),
    );
  }
});

test("opt-in real coding conversation and portfolio capture", async ({
  page,
}, info) => {
  test.skip(
    process.env.CACHE_CAB_CODING_TEST !== "1",
    "Requires the approved dedicated Qwen coding engine",
  );
  test.setTimeout(240000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?view=coding");
  await expect(page.locator("#coding-reset")).toBeEnabled();
  await page.locator("#coding-reset").click();
  await expect(page.locator("#coding-next")).toHaveText("Generate refactor");
  const before = await (await page.request.get("/api/state")).json();
  const frames = path.resolve("captures/coding");
  await mkdir(frames, { recursive: true });
  const region = page.locator("#coding-frame");
  const initial = await region.boundingBox();
  if (!initial) throw new Error("Coding frame is not visible");
  const trace: CodingReport[] = [];
  const capture = async (index: number) => {
    const box = await region.boundingBox();
    if (!box) throw new Error("Coding frame is not visible");
    expect({ width: box.width, height: box.height }).toEqual({
      width: initial.width,
      height: initial.height,
    });
    if (info.project.name === "desktop")
      await region.screenshot({
        path: path.join(frames, `frame-${index}.png`),
        animations: "disabled",
      });
  };
  await capture(0);
  for (const [index, command, stage] of [
    [1, "Generate refactor", "answered"],
    [2, "Save + clear slot", "checkpointed"],
    [3, "Restore + follow-up", "complete"],
  ] as const) {
    const received = page.waitForResponse((response) =>
      response.url().endsWith("/api/coding/step"),
    );
    await page.getByRole("button", { name: command, exact: true }).click();
    const response = await received;
    const report: CodingReport = await response.json();
    expect(response.ok(), JSON.stringify(report)).toBe(true);
    expect(report.stage).toBe(stage);
    trace.push(report);
    await expect(page.locator("#coding-next-label")).not.toHaveText(
      "Working locally",
    );
    await capture(index);
  }
  const report = trace.at(-1) as Completed<CodingReport>;
  expect(report.identity.model_sha256).toBe(
    "cc324af070c2ecbfd324a30884d2f951a7ff756aba85cb811a6ec436933bb046",
  );
  expect(report.first.engine.content.length).toBeGreaterThan(0);
  expect(report.followup.engine.content.length).toBeGreaterThan(0);
  expect(report.first.engine.stop_type).toBe("eos");
  expect(report.validation.passed, report.validation.details).toBe(true);
  expect(report.validation.cases).toBe(12);
  expect(report.first.engine.content).toContain("invoice_total");
  expect(report.output_limit).toBe(256);
  expect(report.slot_cleared).toBe(true);
  expect(report.save.engine.bytes).toBeGreaterThan(0);
  expect(report.restore.engine.n_read).toBe(report.save.engine.bytes);
  expect(report.followup.engine.timings.cache_n).toBeGreaterThan(0);
  expect(report.followup.engine.timings.prompt_n).toBeGreaterThan(0);
  await expect(page.locator("#coding-answer")).toHaveText(
    report.first.engine.content,
  );
  await expect(page.locator("#coding-followup")).toHaveText(
    report.followup.engine.content,
  );
  await expect(page.locator("#coding-reference")).toHaveText(
    report.followup_reference,
  );
  expect(report.followup_reference).toContain("Some(273)");
  expect((await (await page.request.get("/api/state")).json()).step).toBe(
    before.step,
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth,
    ),
  ).toBe(false);
  expect(
    await page
      .locator("img:visible")
      .evaluateAll((images) =>
        images.every(
          (image) =>
            image instanceof HTMLImageElement &&
            image.complete &&
            image.naturalWidth > 0,
        ),
      ),
  ).toBe(true);
  expect(errors).toEqual([]);
  await region.screenshot({
    path: path.resolve(`assets/coding-${info.project.name}.png`),
    animations: "disabled",
  });
  if (info.project.name === "desktop") {
    await writeFile(
      path.join(frames, "trace.json"),
      JSON.stringify(trace, null, 2),
    );
    await writeFile(
      path.resolve("assets/coding-report.json"),
      JSON.stringify(report, null, 2),
    );
  }
});

async function reset(page: Page, budget = 8) {
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Reset route", exact: true }),
  ).toBeEnabled();
  await page.locator("#budget").evaluate((slider: HTMLInputElement, value) => {
    slider.value = String(value);
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
  }, budget);
  await expect(page.locator("#budget-value")).toHaveText(`${budget} units`);
  await expect(page.locator("#request-number")).toHaveText("00");
  await expect(
    page.getByRole("button", { name: "Next request", exact: true }),
  ).toBeEnabled();
}

async function step(page: Page, index: number) {
  await page.getByRole("button", { name: "Next request", exact: true }).click();
  await expect(page.locator("#request-number")).toHaveText(
    String(index).padStart(2, "0"),
  );
}

test("same trace preserves expensive context under cost-aware retention", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await reset(page);
  for (let index = 1; index <= 4; index += 1) await step(page, index);
  await expect(page.locator("#lru-decision")).toContainText("Disk restore");
  await expect(page.locator("#cost-decision")).toContainText("RAM hit");
  await expect(page.locator("#time-delta")).toHaveText("160 ms");
  const state: Snapshot = await (await page.request.get("/api/state")).json();
  expect(state.checkpoint_files).toBeGreaterThan(0);
  expect(state.checkpoint_bytes).toBeGreaterThan(0);
  expect(state.policies[0].stats.requests).toBe(
    state.policies[1].stats.requests,
  );
  expect(
    await page
      .locator("img")
      .evaluateAll((images) =>
        images.every(
          (image) =>
            image instanceof HTMLImageElement &&
            image.complete &&
            image.naturalWidth > 0,
        ),
      ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("playback pauses, resumes, completes, and resets", async ({ page }) => {
  await reset(page);
  await page.getByRole("button", { name: "Run route", exact: true }).click();
  await expect(page.locator("#request-number")).not.toHaveText("00");
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  await expect(page.locator("#request-state")).toHaveText("PAUSED");
  await expect(
    page.getByRole("button", { name: "Next request", exact: true }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Run route", exact: true }).click();
  await expect(page.locator("#request-number")).toHaveText("12", {
    timeout: 20000,
  });
  await expect(
    page.getByRole("button", { name: "Replay route", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Next request", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Reset route", exact: true }).click();
  await expect(page.locator("#request-number")).toHaveText("00");
  await expect(page.locator("#checkpoint-count")).toHaveText("0 files");
});

test("responsive layout and budget controls remain usable", async ({
  page,
}) => {
  await reset(page, 12);
  for (let index = 1; index <= 3; index += 1) await step(page, index);
  const layout = await page.evaluate(() => ({
    overflow: document.documentElement.scrollWidth > innerWidth,
    clipped: [
      ...document.querySelectorAll(
        ".session-tile strong,.session-tile span,button,.metric-label,.policy-title",
      ),
    ]
      .filter((element) => element.scrollWidth > element.clientWidth + 1)
      .map((element) => element.textContent),
  }));
  expect(layout).toEqual({ overflow: false, clipped: [] });
  await expect(page.locator("#lru-capacity")).toHaveText("10 / 12 units");
  await reset(page, 6);
  for (let index = 1; index <= 12; index += 1) await step(page, index);
  const state: Snapshot = await (await page.request.get("/api/state")).json();
  expect(state.policies.every((policy) => policy.used_units <= 6)).toBe(true);
});

test("capture portfolio frames from the actual demo", async ({
  page,
}, info) => {
  test.skip(
    info.project.name !== "desktop",
    "Portfolio GIF uses a fixed desktop frame",
  );
  await reset(page);
  const frames = path.resolve("captures");
  const assets = path.resolve("assets");
  await mkdir(frames, { recursive: true });
  await mkdir(assets, { recursive: true });
  const region = page.locator("#demo-frame");
  const initial = await region.boundingBox();
  if (!initial) throw new Error("Simulation frame is not visible");
  const trace: Snapshot[] = [];
  for (let index = 0; index <= 12; index += 1) {
    if (index > 0) await step(page, index);
    const box = await region.boundingBox();
    if (!box) throw new Error("Simulation frame is not visible");
    expect({ width: box.width, height: box.height }).toEqual({
      width: initial.width,
      height: initial.height,
    });
    await region.screenshot({
      path: path.join(frames, `frame-${String(index).padStart(2, "0")}.png`),
      animations: "disabled",
    });
    trace.push(await (await page.request.get("/api/state")).json());
  }
  await region.screenshot({
    path: path.join(assets, "demo-desktop.png"),
    animations: "disabled",
  });
  await writeFile(
    path.join(frames, "trace.json"),
    JSON.stringify(trace, null, 2),
  );
  expect(trace.at(-1)?.step).toBe(12);
  expect(trace.at(-1)?.mode).toBe("deterministic_simulation");
});

test("opt-in live engine restores actual KV state without changing the simulation", async ({
  page,
}, info) => {
  test.skip(
    process.env.CACHE_CAB_LIVE_TEST !== "1",
    "Requires an explicitly configured dedicated local llama.cpp server",
  );
  await page.goto("/");
  await expect(page.locator("#live-run")).toBeEnabled();
  const before = await (await page.request.get("/api/state")).json();
  const result = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/live/probe") &&
      response.request().method() === "POST",
  );
  await page
    .getByRole("button", { name: "Run checkpoint test", exact: true })
    .click();
  const response = await result;
  expect(response.ok()).toBe(true);
  const report: ProbeReport = await response.json();
  expect(report.mode).toBe("real_llama_cpp_checkpoint_smoke_test");
  expect(report.verified).toBe(true);
  expect(report.restore.engine.n_read).toBe(report.save.engine.bytes);
  expect(report.reused_prefill.engine.timings.cache_n).toBeGreaterThan(0);
  expect(report.reused_prefill.engine.timings.prompt_n).toBeLessThan(
    report.cold_prefill.engine.timings.prompt_n,
  );
  expect(report.continuation_tokens).toBeGreaterThan(report.prompt_tokens);
  expect(report.continued_prefill.engine.timings.cache_n).toBeGreaterThan(0);
  expect(report.continued_prefill.engine.timings.prompt_n).toBeGreaterThan(0);
  await expect(page.locator("#live-status")).toContainText("/ verified");
  await expect(page.locator("#live-cached-tokens")).toHaveText(
    `${report.reused_prefill.engine.timings.cache_n} reused / ${report.reused_prefill.engine.timings.prompt_n} evaluated`,
  );
  await expect(page.locator("#live-verdict")).toContainText("Continuation:");
  await expect(page.locator(".simulation")).toHaveText("DETERMINISTIC DEMO");
  const after = await (await page.request.get("/api/state")).json();
  expect(after.step).toBe(before.step);
  const layout = await page.evaluate(() => ({
    overflow: document.documentElement.scrollWidth > innerWidth,
    clipped: [
      ...document.querySelectorAll(".live-metrics strong,.live-heading button"),
    ]
      .filter((node) => node.scrollWidth > node.clientWidth + 1)
      .map((node) => node.textContent),
  }));
  expect(layout).toEqual({ overflow: false, clipped: [] });
  await page
    .locator(".live-lab")
    .screenshot({
      path: path.resolve(`assets/live-${info.project.name}.png`),
      animations: "disabled",
    });
});
