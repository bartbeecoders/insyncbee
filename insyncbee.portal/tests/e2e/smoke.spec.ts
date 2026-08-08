import { expect, test } from "@playwright/test";

// These smoke tests run against the production Vite build served by `vite
// preview`. They catch the kind of bugs that snuck into v0.1.0–v0.1.4 (wrong
// download filenames, missing recommended download, unbuildable bundle) and
// the v0.2.1 one: the page offering only the headless service, with no way to
// get the app that has the UI.

test("landing page renders the hero and download section", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveTitle(/InSyncBee/i);
  await expect(page.getByRole("heading", { name: /Get InSyncBee/i })).toBeVisible();
});

test("both products are offered", async ({ page }) => {
  await page.goto("/#download");
  await expect(
    page.getByRole("heading", { name: "Desktop app", level: 3, exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "db-service", level: 3, exact: true }),
  ).toBeVisible();
});

test("the desktop app offers Linux bundles a user can actually run", async ({ page }) => {
  await page.goto("/#download");
  const desktop = page.locator("#download-desktop");
  await expect(desktop.getByRole("heading", { name: "Linux", level: 4 })).toBeVisible();
  for (const ext of ["AppImage", "deb", "rpm"]) {
    await expect(desktop.locator(`a[href$=".${ext}"]`)).toHaveCount(1);
  }
});

test("db-service cards include all three platforms", async ({ page }) => {
  await page.goto("/#download");
  const svc = page.locator("#download-db-service");
  for (const heading of ["Linux", "macOS", "Windows"]) {
    await expect(
      svc.getByRole("heading", { name: heading, level: 4, exact: true }),
    ).toBeVisible();
  }
});

test("each download link points to /releases/<file> with the right shape", async ({ page }) => {
  await page.goto("/#download");
  const hrefs = await page.locator("a[href^='/releases/']").evaluateAll((els) =>
    els.map((e) => (e as HTMLAnchorElement).getAttribute("href")),
  );
  // 3 db-service archives + 3 desktop bundles, plus the recommended button.
  expect(hrefs.length).toBeGreaterThanOrEqual(6);
  for (const href of hrefs) {
    expect(href).toMatch(
      /^\/releases\/insyncbee-(db-service-\d+\.\d+\.\d+-(linux-x86_64\.tar\.gz|macos-aarch64\.tar\.gz|windows-x86_64\.zip)|desktop-\d+\.\d+\.\d+-linux-x86_64\.(AppImage|deb|rpm))$/,
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
