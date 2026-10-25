import { expect, test, type Page } from "@playwright/test";

import { loginToDisposableServer } from "./admin-flow";
import { startDisposableServer } from "./server-harness";

/**
 * Home keeps following Reports across a Server restart. The realtime sequence
 * lives in memory, so a restarted Server begins a new one while the open tab
 * still holds the cursor the previous process issued. The tab must be told to
 * reset and resume from the new process instead of waiting in silence until
 * the new sequence happens to climb past the cursor it kept — on a deployment
 * that ran for days, that wait was indistinguishable from a frozen Home.
 */

function reportAtHeight(sequence: number, height: number) {
  return (report: Record<string, unknown>) => {
    const observed = new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
    report.report_sequence = sequence;
    report.report_id =
      "0195f2a1-01b7-4091-8091-0000000000" + String(sequence).padStart(2, "0");
    report.generated_at = observed;
    const node = (report.nodes as Record<string, unknown>[])[0];
    const sync = (node.chain as Record<string, unknown>).sync as Record<
      string,
      unknown
    >;
    sync.attempted_at = observed;
    sync.latest_observed_at = observed;
    const latest = sync.latest as Record<string, unknown>;
    latest.current_block = height;
    latest.highest_block = height;
  };
}

/** The height as Home may print it, with or without digit grouping. */
function heightText(height: number): RegExp {
  return new RegExp(
    String(height).replace(/(\d)(?=(\d{3})+$)/g, "$1[,.\\s\u00a0\u202f]?"),
  );
}

async function expectHomeShowsHeight(
  page: Page,
  height: number,
): Promise<void> {
  await expect(page.getByRole("region", { name: "Home" })).toContainText(
    heightText(height),
    {
      timeout: 15_000,
    },
  );
}

test("Home follows new Reports after the Server restarts", async ({
  page,
}, info) => {
  test.skip(
    info.project.name !== "desktop-1280",
    "one restart proves the stream recovery",
  );
  test.setTimeout(120_000);
  const server = await startDisposableServer();
  try {
    const agent = await server.enrollAgent();
    let sequence = 1;
    await server.submitReport(
      agent.agentId,
      agent.credential,
      reportAtHeight(sequence, 1000 + sequence),
    );
    await loginToDisposableServer(page, server.baseUrl);
    await page.goto(server.baseUrl + "/");
    await expectHomeShowsHeight(page, 1001);

    // Give the tab a cursor well into the first process's sequence.
    for (sequence = 2; sequence <= 12; sequence += 1) {
      await server.submitReport(
        agent.agentId,
        agent.credential,
        reportAtHeight(sequence, 1000 + sequence),
      );
    }
    await expectHomeShowsHeight(page, 1012);

    // Report only once the tab is back on a stream, so the new height can
    // reach Home through that stream and not through the refetch that follows
    // the restart itself.
    const reconnected = page.waitForResponse(
      (response) =>
        response.url().includes("/api/public/v1/events") &&
        response.status() === 200,
      { timeout: 30_000 },
    );
    await server.restart();
    await reconnected;
    await page.waitForTimeout(1_000);
    await server.submitReport(
      agent.agentId,
      agent.credential,
      reportAtHeight(sequence, 1000 + sequence),
    );
    await expectHomeShowsHeight(page, 1000 + sequence);
  } finally {
    await server.dispose();
  }
});
