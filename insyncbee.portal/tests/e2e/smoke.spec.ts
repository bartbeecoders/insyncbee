import { expect, test } from "@playwright/test";

// These smoke tests run against the production Vite build served by `vite
// preview`. They catch the kind of bugs that snuck into v0.1.0–v0.1.4 (wrong
// download filenames, missing recommended download, unbuildable bundle).

test("landing page renders the hero and download section", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveTitle(/InSyncBee/i);
  await expect(page.getByRole("heading", { name: /Get the InSyncBee db-service/i })).toBeVisible();
});

test("download cards include all three platforms", async ({ page }) => {
  await page.goto("/#download");
  for (const heading of ["Linux", "macOS", "Windows"]) {
    // exact: true so we don't also match the "We detected <Platform>" heading.
    await expect(
      page.getByRole("heading", { name: heading, level: 3, exact: true }),
    ).toBeVisible();
  }
});

test("each download link points to /releases/<file> with the right shape", async ({ page }) => {
  await page.goto("/#download");
  const hrefs = await page.locator("a[href^='/releases/']").evaluateAll((els) =>
    els.map((e) => (e as HTMLAnchorElement).getAttribute("href")),
  );
  // Expect at least one link per platform.
  expect(hrefs.length).toBeGreaterThanOrEqual(3);
  for (const href of hrefs) {
    expect(href).toMatch(
      /^\/releases\/insyncbee-db-service-\d+\.\d+\.\d+-(linux-x86_64\.tar\.gz|macos-aarch64\.tar\.gz|windows-x86_64\.zip)$/,
    );
  }
});

test("github release footnote points at the matching tag", async ({ page }) => {
  await page.goto("/#download");
  const link = page.getByRole("link", { name: /v\d+\.\d+\.\d+ GitHub Release/i });
  await expect(link).toBeVisible();
  const href = await link.getAttribute("href");
  expect(href).toMatch(/^https:\/\/github\.com\/[^/]+\/[^/]+\/releases\/tag\/v\d+\.\d+\.\d+$/);
});
