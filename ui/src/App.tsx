import { createSignal, createResource, Show, For, onMount, onCleanup, JSX } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import changelogMd from "../../CHANGELOG.md?raw";

const APP_VERSION = "0.2.7";

type TransferKind = "upload" | "download";

interface TransferProgress {
  syncPairId: string;
  kind: TransferKind;
  name: string;
  path: string;
  bytes: number;
  total: number;
}

// Augmented entry stored client-side. We snapshot the first event's
// (time, bytes) so we can compute average speed and ETA from the deltas
// — the backend doesn't send timing info, just current/total.
interface UploadEntry extends TransferProgress {
  startedAt: number;
  startedBytes: number;
  lastUpdatedAt: number;
  speedBps: number | null;
  etaSeconds: number | null;
}

type UploadsByPair = Record<string, Record<string, UploadEntry>>;

type SyncPhase =
  | "scanning-local"
  | "listing-remote"
  | "comparing"
  | "executing"
  | "finished";

interface SyncStatus {
  phase: SyncPhase;
  done: number;
  total: number;
  action: string | null;
  path: string | null;
}

interface SyncStatusPayload {
  syncPairId: string;
  status: SyncStatus;
}

type SyncStatusByPair = Record<string, SyncStatus>;

// What each phase is called in the UI. Scanning and listing take real time
// on a large pair and move no bytes, which is exactly when a sync used to
// look like it had stalled.
const PHASE_LABEL: Record<SyncPhase, string> = {
  "scanning-local": "Scanning local files",
  "listing-remote": "Listing Drive files",
  comparing: "Comparing",
  executing: "Syncing",
  finished: "Finishing up",
};

interface Account {
  id: string;
  email: string;
  display_name: string | null;
}

interface SyncPair {
  id: string;
  name: string;
  account_id: string;
  local_root: string;
  remote_root_id: string;
  remote_root_path: string;
  mode: string;
  status: string;
  conflict_policy: string;
  poll_interval_secs: number;
  encryption_enabled: boolean;
}

interface DriveFolder {
  id: string;
  name: string;
}

interface LocalFolder {
  name: string;
  path: string;
}

interface FileEntry {
  id: number;
  sync_pair_id: string;
  relative_path: string;
  local_hash: string | null;
  remote_md5: string | null;
  remote_id: string | null;
  state: string;
}

interface ChangeLogEntry {
  id: number;
  sync_pair_id: string;
  relative_path: string;
  action: string;
  detail: string | null;
  created_at: string;
  // Null for non-transfer actions, and for transfers logged before
  // byte accounting landed in schema v3.
  bytes: number | null;
  duration_ms: number | null;
}

interface DirectionTotals {
  files: number;
  bytes: number;
  duration_ms: number;
  measured_files: number;
}

interface TransferStats {
  uploaded: DirectionTotals;
  downloaded: DirectionTotals;
  deleted: number;
  conflicts: number;
  errors: number;
  since: string | null;
  last_activity: string | null;
}

interface StatsPayload {
  allTime: TransferStats;
  last7Days: TransferStats;
}

interface PairStats {
  syncPairId: string;
  name: string;
  stats: TransferStats;
}

type Tab = "dashboard" | "activity" | "statistics" | "conflicts" | "settings" | "about";

function App() {
  const [tab, setTab] = createSignal<Tab>("dashboard");
  const [accounts, { refetch: refetchAccounts }] = createResource(fetchAccounts);
  const [syncPairs, { refetch: refetchPairs }] = createResource(fetchSyncPairs);
  const [selectedPair, setSelectedPair] = createSignal<string | null>(null);

  // Per-sync-pair, per-file upload progress map. Populated by Tauri events
  // emitted from the resumable-upload chunk loop; cleared per pair when
  // that pair's sync finishes (which is the natural moment to drop bars,
  // whether the sync succeeded or errored mid-flight).
  const [uploads, setUploads] = createStore<UploadsByPair>({});
  // Which phase each pair's in-flight sync is in. Keyed by pair so two
  // pairs syncing at once don't overwrite each other's indicator.
  const [syncStatus, setSyncStatus] = createStore<SyncStatusByPair>({});

  onMount(() => {
    const unlistenStatus = listen<SyncStatusPayload>("sync-status", (e) => {
      const { syncPairId, status } = e.payload;
      setSyncStatus(produce((s) => { s[syncPairId] = status; }));
    });
    const unlistenProgress = listen<TransferProgress>("transfer-progress", (e) => {
      const p = e.payload;
      const now = Date.now();
      setUploads(
        produce((u) => {
          if (!u[p.syncPairId]) u[p.syncPairId] = {};
          const prev = u[p.syncPairId][p.path];
          // Anchor speed/ETA calculations to the first event we see for
          // this file. We can't use bytes=0 as a sentinel because in-flight
          // uploads from a previous sync could resume at non-zero offsets.
          const startedAt = prev?.startedAt ?? now;
          const startedBytes = prev?.startedBytes ?? p.bytes;
          const elapsedMs = now - startedAt;
          const transferred = p.bytes - startedBytes;
          // Average speed since start. Resilient — doesn't oscillate on a
          // slow chunk the way a sliding-window estimate would. The
          // tradeoff: ETA reacts slowly if the network changes mid-upload,
          // which for personal-file sync is a non-issue.
          const speedBps =
            elapsedMs > 250 && transferred > 0
              ? (transferred * 1000) / elapsedMs
              : null;
          const remaining = Math.max(0, p.total - p.bytes);
          const etaSeconds =
            speedBps && speedBps > 0 ? remaining / speedBps : null;
          u[p.syncPairId][p.path] = {
            ...p,
            startedAt,
            startedBytes,
            lastUpdatedAt: now,
            speedBps,
            etaSeconds,
          };
        }),
      );
    });
    const unlistenFinished = listen<string>("sync-finished", (e) => {
      const pairId = e.payload;
      setUploads(produce((u) => { delete u[pairId]; }));
      setSyncStatus(produce((s) => { delete s[pairId]; }));
    });
    onCleanup(() => {
      unlistenProgress.then((u) => u());
      unlistenFinished.then((u) => u());
      unlistenStatus.then((u) => u());
    });
  });

  async function fetchAccounts(): Promise<Account[]> {
    return await invoke("list_accounts");
  }

  async function fetchSyncPairs(): Promise<SyncPair[]> {
    return await invoke("list_sync_pairs");
  }

  return (
    <div class="app">
      <header class="header">
        <div class="logo">
          <span class="logo-icon">B</span>
          <h1>InSyncBee</h1>
        </div>
        <nav class="tabs">
          <button
            class={tab() === "dashboard" ? "tab active" : "tab"}
            onClick={() => setTab("dashboard")}
          >
            Dashboard
          </button>
          <button
            class={tab() === "activity" ? "tab active" : "tab"}
            onClick={() => setTab("activity")}
          >
            Activity
          </button>
          <button
            class={tab() === "statistics" ? "tab active" : "tab"}
            onClick={() => setTab("statistics")}
          >
            Statistics
          </button>
          <button
            class={tab() === "conflicts" ? "tab active" : "tab"}
            onClick={() => setTab("conflicts")}
          >
            Conflicts
          </button>
          <button
            class={tab() === "settings" ? "tab active" : "tab"}
            onClick={() => setTab("settings")}
          >
            Settings
          </button>
          <button
            class={tab() === "about" ? "tab active" : "tab"}
            onClick={() => setTab("about")}
          >
            About
          </button>
        </nav>
      </header>

      <main class="main">
        <Show when={tab() === "dashboard"}>
          <Dashboard
            accounts={accounts() ?? []}
            syncPairs={syncPairs() ?? []}
            uploads={uploads}
            syncStatus={syncStatus}
            onRefresh={() => { refetchAccounts(); refetchPairs(); }}
            onSelectPair={setSelectedPair}
          />
        </Show>
        <Show when={tab() === "activity"}>
          <ActivityFeed
            syncPairs={syncPairs() ?? []}
            selectedPair={selectedPair()}
            uploads={uploads}
          />
        </Show>
        <Show when={tab() === "statistics"}>
          <StatisticsView syncPairs={syncPairs() ?? []} />
        </Show>
        <Show when={tab() === "conflicts"}>
          <ConflictsView
            syncPairs={syncPairs() ?? []}
            selectedPair={selectedPair()}
          />
        </Show>
        <Show when={tab() === "settings"}>
          <SettingsView />
        </Show>
        <Show when={tab() === "about"}>
          <AboutView />
        </Show>
      </main>
    </div>
  );
}

function AboutView() {
  return (
    <section class="section">
      <div class="section-header">
        <h2>About InSyncBee</h2>
      </div>
      <div class="about-meta">
        <div>
          <span class="about-label">Version</span>
          <span class="about-value">{APP_VERSION}</span>
        </div>
        <div>
          <span class="about-label">License</span>
          <span class="about-value">MIT</span>
        </div>
        <div>
          <span class="about-label">Source</span>
          <a
            class="about-link"
            href="https://github.com/bartbeecoders/insyncbee"
            target="_blank"
            rel="noreferrer"
          >
            github.com/bartbeecoders/insyncbee
          </a>
        </div>
      </div>
      <h3 class="about-changelog-heading">Changelog</h3>
      <Changelog source={changelogMd} />
    </section>
  );
}

// Tiny renderer for the small subset of Markdown the changelog uses:
// `## v…` headings, `### Added/Fixed/Changed` sub-headings, `- ` bullet
// lists (with continuation lines), paragraphs, and `**bold**` inline.
// Everything before the first `## ` heading (the file's intro paragraph)
// is dropped because it doesn't belong on an in-app About page.
function Changelog(props: { source: string }) {
  const lines = props.source.split("\n");
  const start = lines.findIndex((l) => l.startsWith("## "));
  const body = start >= 0 ? lines.slice(start) : lines;

  const blocks: JSX.Element[] = [];
  let i = 0;
  while (i < body.length) {
    const line = body[i];
    if (line.startsWith("## ")) {
      blocks.push(<h4 class="changelog-version">{line.slice(3)}</h4>);
      i++;
    } else if (line.startsWith("### ")) {
      blocks.push(<h5 class="changelog-section">{line.slice(4)}</h5>);
      i++;
    } else if (line.startsWith("- ")) {
      const items: string[] = [];
      while (
        i < body.length &&
        (body[i].startsWith("- ") || body[i].startsWith("  "))
      ) {
        if (body[i].startsWith("- ")) {
          items.push(body[i].slice(2));
        } else if (items.length > 0) {
          items[items.length - 1] += " " + body[i].trim();
        }
        i++;
      }
      blocks.push(
        <ul class="changelog-list">
          <For each={items}>{(it) => <li>{renderInline(it)}</li>}</For>
        </ul>,
      );
    } else if (line.trim() === "") {
      i++;
    } else {
      const para: string[] = [line];
      i++;
      while (
        i < body.length &&
        body[i].trim() !== "" &&
        !body[i].startsWith("#") &&
        !body[i].startsWith("- ")
      ) {
        para.push(body[i]);
        i++;
      }
      blocks.push(<p class="changelog-p">{renderInline(para.join(" "))}</p>);
    }
  }
  return <div class="changelog">{blocks}</div>;
}

function renderInline(text: string): JSX.Element {
  const out: JSX.Element[] = [];
  const re = /\*\*([^*]+)\*\*/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(<strong>{m[1]}</strong>);
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

function SettingsView() {
  const [autostart, setAutostart] = createSignal<boolean | null>(null);
  const [autoSync, setAutoSync] = createSignal<boolean | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  onMount(async () => {
    try {
      setAutostart(await invoke<boolean>("autostart_enabled"));
      setAutoSync(await invoke<boolean>("get_auto_sync"));
    } catch (e) {
      setError(String(e));
    }
  });

  async function toggleAutoSync() {
    const next = !autoSync();
    setBusy(true);
    setError(null);
    try {
      await invoke("set_auto_sync", { enabled: next });
      setAutoSync(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function toggle() {
    const next = !autostart();
    setBusy(true);
    setError(null);
    try {
      await invoke(next ? "autostart_enable" : "autostart_disable");
      setAutostart(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="section">
      <div class="section-header">
        <h2>Settings</h2>
      </div>
      <div class="settings-row">
        <div>
          <div class="settings-label">Sync automatically</div>
          <div class="settings-help">
            Keep sync pairs up to date in the background: an initial sync when
            the app starts, immediately when local files change, and on each
            pair's poll interval for changes made on Drive. Turn this off to
            sync only when you press Sync Now.
          </div>
        </div>
        <label class="toggle">
          <input
            type="checkbox"
            checked={autoSync() === true}
            disabled={autoSync() === null || busy()}
            onChange={toggleAutoSync}
          />
          <span>{autoSync() ? "On" : "Off"}</span>
        </label>
      </div>
      <div class="settings-row">
        <div>
          <div class="settings-label">Start on login</div>
          <div class="settings-help">
            Launch InSyncBee in the background (tray only) when you log in.
          </div>
        </div>
        <label class="toggle">
          <input
            type="checkbox"
            checked={autostart() === true}
            disabled={autostart() === null || busy()}
            onChange={toggle}
          />
          <span>{autostart() ? "On" : "Off"}</span>
        </label>
      </div>
      <Show when={error()}>
        <div class="error">{error()}</div>
      </Show>
    </section>
  );
}

function Dashboard(props: {
  accounts: Account[];
  syncPairs: SyncPair[];
  uploads: UploadsByPair;
  syncStatus: SyncStatusByPair;
  onRefresh: () => void;
  onSelectPair: (id: string | null) => void;
}) {
  const [loggingIn, setLoggingIn] = createSignal(false);
  const [loginError, setLoginError] = createSignal<string | null>(null);
  const [syncing, setSyncing] = createSignal<string | null>(null);
  const [editingPair, setEditingPair] = createSignal<SyncPair | null>(null);
  const [showForm, setShowForm] = createSignal(false);
  const [deleting, setDeleting] = createSignal<string | null>(null);
  const [reconnecting, setReconnecting] = createSignal<string | null>(null);
  const [unlockingPair, setUnlockingPair] = createSignal<SyncPair | null>(null);

  function openAddForm() {
    setEditingPair(null);
    setShowForm(true);
  }

  function openEditForm(pair: SyncPair) {
    setEditingPair(pair);
    setShowForm(true);
  }

  async function handleDelete(pair: SyncPair) {
    const ok = confirm(
      `Delete sync pair "${pair.name}"?\n\nThis stops syncing and removes its history from the database. Local and Drive files are NOT deleted.`,
    );
    if (!ok) return;
    setDeleting(pair.id);
    try {
      await invoke("delete_sync_pair", { id: pair.id });
      props.onRefresh();
    } catch (e) {
      alert(`Failed to delete: ${e}`);
    } finally {
      setDeleting(null);
    }
  }

  async function handleLogin() {
    setLoggingIn(true);
    setLoginError(null);
    try {
      await invoke("start_login");
      props.onRefresh();
    } catch (e) {
      setLoginError(String(e));
    } finally {
      setLoggingIn(false);
    }
  }

  async function handleLogout(acc: Account) {
    const ok = confirm(
      `Remove account "${acc.email}"?\n\nThis disconnects the account from InSyncBee. Files on Google Drive and on disk are NOT affected.`,
    );
    if (!ok) return;
    try {
      await invoke("logout", { accountId: acc.id });
      props.onRefresh();
    } catch (e) {
      alert(`Failed to remove account: ${e}`);
    }
  }

  async function handleReconnect(acc: Account) {
    setReconnecting(acc.id);
    try {
      await invoke("reconnect_account", { accountId: acc.id });
      props.onRefresh();
    } catch (e) {
      alert(`Failed to reconnect: ${e}`);
    } finally {
      setReconnecting(null);
    }
  }

  // A pair counts as syncing while this window is running its sync, or
  // while phase events are still arriving — the button-click state alone
  // misses the window between "sync started" and the first byte moving.
  const isSyncing = (pairId: string) =>
    syncing() === pairId ||
    (props.syncStatus[pairId] != null &&
      props.syncStatus[pairId].phase !== "finished");

  async function handleSync(pair: SyncPair) {
    // Encrypted pair? Make sure the key is in the keyring before kicking
    // off — otherwise the daemon would error per-file. Prompt for the
    // passphrase if needed, then proceed.
    if (pair.encryption_enabled) {
      const unlocked = await invoke<boolean>("encryption_unlocked", {
        syncPairId: pair.id,
      });
      if (!unlocked) {
        setUnlockingPair(pair);
        return;
      }
    }

    setSyncing(pair.id);
    try {
      const report = await invoke<string>("trigger_sync", { syncPairId: pair.id });
      console.log("Sync result:", report);
      props.onRefresh();
    } catch (e) {
      console.error("Sync failed:", e);
    } finally {
      setSyncing(null);
    }
  }

  return (
    <div class="dashboard">
      <section class="section">
        <div class="section-header">
          <h2>Accounts</h2>
          <button
            class="btn btn-sm"
            onClick={handleLogin}
            disabled={loggingIn()}
          >
            {loggingIn() ? "Signing in..." : "Add Account"}
          </button>
        </div>
        <Show when={loginError()}>
          <p class="error-msg">{loginError()}</p>
        </Show>
        <Show
          when={props.accounts.length > 0}
          fallback={
            <p class="empty">No accounts connected. Click "Add Account" to sign in with Google.</p>
          }
        >
          <div class="card-list">
            <For each={props.accounts}>
              {(acc) => (
                <div class="card">
                  <div class="card-header">
                    <div class="card-title">{acc.email}</div>
                    <div class="section-actions">
                      <button
                        class="btn btn-sm btn-ghost"
                        onClick={() => handleReconnect(acc)}
                        disabled={reconnecting() === acc.id}
                        title="Re-run Google sign-in to refresh tokens (use this if syncing fails with an auth error)"
                      >
                        {reconnecting() === acc.id ? "Reconnecting..." : "Reconnect"}
                      </button>
                      <button
                        class="btn btn-sm btn-ghost btn-danger"
                        onClick={() => handleLogout(acc)}
                      >
                        Remove
                      </button>
                    </div>
                  </div>
                  <div class="card-subtitle">{acc.display_name ?? "Google Account"}</div>
                </div>
              )}
            </For>
          </div>
        </Show>
      </section>

      <section class="section">
        <div class="section-header">
          <h2>Sync Pairs</h2>
          <div class="section-actions">
            <button
              class="btn btn-sm btn-primary"
              disabled={props.accounts.length === 0}
              title={
                props.accounts.length === 0
                  ? "Connect a Google account first"
                  : "Add a new sync pair"
              }
              onClick={openAddForm}
            >
              + Add Sync Pair
            </button>
            <button class="btn btn-sm btn-ghost" onClick={props.onRefresh}>
              Refresh
            </button>
          </div>
        </div>
        <Show
          when={props.syncPairs.length > 0}
          fallback={
            <p class="empty">
              <Show
                when={props.accounts.length > 0}
                fallback="Connect a Google account above, then add a sync pair."
              >
                No sync pairs configured. Click "+ Add Sync Pair" to create one.
              </Show>
            </p>
          }
        >
          <div class="card-list">
            <For each={props.syncPairs}>
              {(pair) => (
                <div
                  class="card card-interactive"
                  classList={{ "card-syncing": isSyncing(pair.id) }}
                  onClick={() => props.onSelectPair(pair.id)}
                >
                  <div class="card-header">
                    <span
                      class={
                        isSyncing(pair.id)
                          ? "status-dot status-syncing"
                          : `status-dot status-${pair.status}`
                      }
                    />
                    <div class="card-title">{pair.name}</div>
                    <span class="badge">{pair.mode}</span>
                    <Show when={isSyncing(pair.id)}>
                      <span class="badge badge-syncing">
                        <span class="spinner" aria-hidden="true" />
                        Syncing
                      </span>
                    </Show>
                  </div>
                  <div class="card-body">
                    <div class="path-row">
                      <span class="label">Local:</span>
                      <code>{pair.local_root}</code>
                    </div>
                    <div class="path-row">
                      <span class="label">Remote:</span>
                      <code>{pair.remote_root_path}</code>
                    </div>
                    <Show when={isSyncing(pair.id)}>
                      <SyncActivity
                        status={props.syncStatus[pair.id]}
                        transfers={Object.values(props.uploads[pair.id] ?? {})}
                      />
                    </Show>
                    <Show when={Object.keys(props.uploads[pair.id] ?? {}).length > 0}>
                      <div class="upload-progress-list">
                        <For each={Object.values(props.uploads[pair.id] ?? {})}>
                          {(up) => <UploadProgressRow upload={up} />}
                        </For>
                      </div>
                    </Show>
                  </div>
                  <div class="card-footer">
                    <button
                      class="btn btn-sm"
                      disabled={syncing() === pair.id}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleSync(pair);
                      }}
                    >
                      {syncing() === pair.id ? "Syncing..." : "Sync Now"}
                    </button>
                    <Show when={pair.encryption_enabled}>
                      <span
                        class="badge badge-encryption"
                        title="Files in this pair are encrypted before upload (AES-256-GCM)"
                      >
                        🔒 Encrypted
                      </span>
                    </Show>
                    <Show when={pair.status === "active"}>
                      <button
                        class="btn btn-sm btn-ghost"
                        onClick={(e) => {
                          e.stopPropagation();
                          invoke("pause_sync_pair", { id: pair.id }).then(
                            props.onRefresh
                          );
                        }}
                      >
                        Pause
                      </button>
                    </Show>
                    <Show when={pair.status === "paused"}>
                      <button
                        class="btn btn-sm btn-ghost"
                        onClick={(e) => {
                          e.stopPropagation();
                          invoke("resume_sync_pair", { id: pair.id }).then(
                            props.onRefresh
                          );
                        }}
                      >
                        Resume
                      </button>
                    </Show>
                    <span class="card-footer-spacer" />
                    <button
                      class="btn btn-sm btn-ghost"
                      onClick={(e) => {
                        e.stopPropagation();
                        openEditForm(pair);
                      }}
                    >
                      Edit
                    </button>
                    <button
                      class="btn btn-sm btn-ghost btn-danger"
                      disabled={deleting() === pair.id}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDelete(pair);
                      }}
                    >
                      {deleting() === pair.id ? "Deleting..." : "Delete"}
                    </button>
                  </div>
                </div>
              )}
            </For>
          </div>
        </Show>
      </section>

      <Show when={showForm()}>
        <SyncPairFormModal
          accounts={props.accounts}
          pair={editingPair()}
          onClose={() => setShowForm(false)}
          onSaved={() => {
            setShowForm(false);
            props.onRefresh();
          }}
        />
      </Show>

      <Show when={unlockingPair()}>
        <UnlockEncryptionModal
          pair={unlockingPair()!}
          onCancel={() => setUnlockingPair(null)}
          onUnlocked={async () => {
            const p = unlockingPair()!;
            setUnlockingPair(null);
            // Now that the key is in the keyring, kick off the sync.
            await handleSync(p);
          }}
        />
      </Show>
    </div>
  );
}

// ── Unlock Encryption Modal ──────────────────────────────────────
//
// Shown when the user clicks "Sync Now" on an encrypted pair whose
// derived key is not currently in the OS keyring. Verifies the
// passphrase server-side via the stored verifier (no key bytes ever
// cross the wire).
function UnlockEncryptionModal(props: {
  pair: SyncPair;
  onCancel: () => void;
  onUnlocked: () => void | Promise<void>;
}) {
  const [passphrase, setPassphrase] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function handleSubmit() {
    if (!passphrase()) {
      setError("Enter the passphrase.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      await invoke("unlock_encryption", {
        syncPairId: props.pair.id,
        passphrase: passphrase(),
      });
      await props.onUnlocked();
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div class="modal-backdrop" onClick={props.onCancel}>
      <div class="modal modal-sm" onClick={(e) => e.stopPropagation()}>
        <div class="modal-header">
          <h2>Unlock "{props.pair.name}"</h2>
          <button class="btn btn-sm btn-ghost" onClick={props.onCancel}>
            ✕
          </button>
        </div>
        <div class="modal-body">
          <p>
            This sync pair is encrypted. Enter the passphrase you set when
            you created it to unlock the key on this machine.
          </p>
          <Show when={error()}>
            <p class="error-msg">{error()}</p>
          </Show>
          <div class="form-field">
            <label>Passphrase</label>
            <input
              type="password"
              autocomplete="current-password"
              autofocus
              value={passphrase()}
              onInput={(e) => setPassphrase(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleSubmit();
              }}
            />
          </div>
        </div>
        <div class="modal-footer">
          <button class="btn btn-ghost" onClick={props.onCancel}>
            Cancel
          </button>
          <button
            class="btn btn-primary"
            disabled={submitting()}
            onClick={handleSubmit}
          >
            {submitting() ? "Unlocking…" : "Unlock & Sync"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Sync Pair Form Modal ──────────────────────────────────────────

const SYNC_MODES: { value: string; label: string }[] = [
  { value: "two-way", label: "Two-way (bidirectional)" },
  { value: "local-to-cloud", label: "Upload only (local → Drive)" },
  { value: "cloud-to-local", label: "Download only (Drive → local)" },
];

function UploadProgressRow(props: { upload: UploadEntry }) {
  // Tick once a second so the ETA visibly counts down between chunk
  // events. On a slow link a chunk can take 30s+ and a static ETA would
  // look frozen. Cleaned up automatically when the row unmounts (which
  // happens when sync-finished clears its pair from the store).
  const [now, setNow] = createSignal(Date.now());
  const interval = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(interval));

  const pct = () =>
    props.upload.total > 0
      ? Math.min(100, Math.round((props.upload.bytes / props.upload.total) * 100))
      : 0;

  // Subtract elapsed-since-last-event from the snapshotted ETA so the
  // displayed value decreases smoothly. Floor at 0 — going negative would
  // mean Drive is taking longer than projected, which we just show as 0s.
  const liveEta = () => {
    const eta = props.upload.etaSeconds;
    if (eta == null) return null;
    const elapsedSec = (now() - props.upload.lastUpdatedAt) / 1000;
    return Math.max(0, eta - elapsedSec);
  };

  const isDone = () => props.upload.bytes >= props.upload.total;

  return (
    <div class="upload-progress" title={props.upload.path}>
      <div class="upload-progress-meta">
        <span class="upload-progress-name">{props.upload.name}</span>
        <span class="upload-progress-bytes">
          {formatBytes(props.upload.bytes)} / {formatBytes(props.upload.total)}
          {" · "}
          {pct()}%
        </span>
      </div>
      <div class="upload-progress-bar">
        <div class="upload-progress-fill" style={{ width: `${pct()}%` }} />
      </div>
      <div class="upload-progress-meta upload-progress-stats">
        <span>
          {props.upload.speedBps != null
            ? `${formatBytes(Math.round(props.upload.speedBps))}/s`
            : "—"}
        </span>
        <span>
          {isDone()
            ? "done"
            : liveEta() != null
              ? `${formatDuration(liveEta()!)} left`
              : "estimating…"}
        </span>
      </div>
    </div>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`;
}

function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds)) return "—";
  const s = Math.round(seconds);
  if (s < 1) return "<1s";
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rs = s % 60;
  if (m < 60) return rs > 0 ? `${m}m ${rs}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  return rm > 0 ? `${h}h ${rm}m` : `${h}h`;
}

const CONFLICT_POLICIES: { value: string; label: string }[] = [
  { value: "keep-both", label: "Keep both (save conflict copy)" },
  { value: "ask", label: "Ask me each time" },
  { value: "prefer-local", label: "Prefer local" },
  { value: "prefer-remote", label: "Prefer remote" },
  { value: "newest-wins", label: "Newest wins (by mtime)" },
];

function SyncPairFormModal(props: {
  accounts: Account[];
  pair: SyncPair | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const editing = () => props.pair !== null;

  const [name, setName] = createSignal(props.pair?.name ?? "");
  const [accountId, setAccountId] = createSignal(
    props.pair?.account_id ?? props.accounts[0]?.id ?? "",
  );
  const [localRoot, setLocalRoot] = createSignal(props.pair?.local_root ?? "");
  const [remoteRootId, setRemoteRootId] = createSignal(
    props.pair?.remote_root_id ?? "root",
  );
  const [remoteRootPath, setRemoteRootPath] = createSignal(
    props.pair?.remote_root_path ?? "/",
  );
  const [mode, setMode] = createSignal(props.pair?.mode ?? "two-way");
  const [conflictPolicy, setConflictPolicy] = createSignal(
    props.pair?.conflict_policy ?? "keep-both",
  );
  const [pollInterval, setPollInterval] = createSignal(
    props.pair?.poll_interval_secs ?? 30,
  );
  const [showLocalPicker, setShowLocalPicker] = createSignal(false);
  const [showRemotePicker, setShowRemotePicker] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  // Encryption is set at creation time. We don't expose a toggle in edit
  // mode because flipping it on/off after files exist would require a
  // re-upload of every file in the pair — out of scope for v1. The user
  // sees a read-only badge in edit mode instead.
  const [encryptionEnabled, setEncryptionEnabled] = createSignal(false);
  const [passphrase, setPassphrase] = createSignal("");
  const [passphraseConfirm, setPassphraseConfirm] = createSignal("");

  function validate(): string | null {
    if (!name().trim()) return "Name is required.";
    if (!editing()) {
      if (!accountId()) return "Select a Google account.";
      if (!localRoot().trim()) return "Pick a local folder.";
      if (!remoteRootId().trim()) return "Pick a Drive folder.";
      if (encryptionEnabled()) {
        if (passphrase().length < 8)
          return "Passphrase must be at least 8 characters.";
        if (passphrase() !== passphraseConfirm())
          return "Passphrases do not match.";
      }
    }
    const interval = pollInterval();
    if (!Number.isFinite(interval) || interval < 5)
      return "Poll interval must be at least 5 seconds.";
    return null;
  }

  async function handleSave() {
    const err = validate();
    if (err) {
      setError(err);
      return;
    }
    setError(null);
    setSaving(true);
    try {
      if (editing()) {
        await invoke("update_sync_pair", {
          id: props.pair!.id,
          name: name().trim(),
          mode: mode(),
          conflictPolicy: conflictPolicy(),
          pollIntervalSecs: pollInterval(),
        });
      } else {
        await invoke("add_sync_pair", {
          name: name().trim(),
          accountId: accountId(),
          localRoot: localRoot().trim(),
          remoteRootId: remoteRootId().trim(),
          remoteRootPath: remoteRootPath().trim() || "/",
          mode: mode(),
          conflictPolicy: conflictPolicy(),
          pollIntervalSecs: pollInterval(),
          encryptionPassphrase: encryptionEnabled() ? passphrase() : null,
        });
      }
      props.onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class="modal-backdrop" onClick={props.onClose}>
      <div class="modal" onClick={(e) => e.stopPropagation()}>
        <div class="modal-header">
          <h2>{editing() ? "Edit Sync Pair" : "Add Sync Pair"}</h2>
          <button class="btn btn-sm btn-ghost" onClick={props.onClose}>
            ✕
          </button>
        </div>

        <div class="modal-body">
          <Show when={error()}>
            <p class="error-msg">{error()}</p>
          </Show>

          <div class="form-field">
            <label>Name</label>
            <input
              type="text"
              value={name()}
              placeholder="My Documents"
              onInput={(e) => setName(e.currentTarget.value)}
            />
          </div>

          <Show when={!editing()}>
            <div class="form-field">
              <label>Google Account</label>
              <select
                value={accountId()}
                onChange={(e) => setAccountId(e.currentTarget.value)}
              >
                <For each={props.accounts}>
                  {(acc) => <option value={acc.id}>{acc.email}</option>}
                </For>
              </select>
            </div>

            <div class="form-field">
              <label>Local Folder</label>
              <div class="form-picker">
                <input
                  type="text"
                  value={localRoot()}
                  placeholder="No folder selected"
                  onInput={(e) => setLocalRoot(e.currentTarget.value)}
                />
                <button class="btn btn-sm" onClick={() => setShowLocalPicker(true)}>
                  Browse…
                </button>
              </div>
            </div>

            <div class="form-field">
              <label>Google Drive Folder</label>
              <div class="form-picker">
                <input
                  type="text"
                  value={remoteRootPath()}
                  placeholder="/"
                  readOnly
                />
                <button
                  class="btn btn-sm"
                  disabled={!accountId()}
                  onClick={() => setShowRemotePicker(true)}
                >
                  Browse…
                </button>
              </div>
              <span class="form-hint">
                Folder ID: <code>{remoteRootId()}</code>
              </span>
            </div>
          </Show>

          <Show when={editing()}>
            <div class="form-field readonly-field">
              <label>Local Folder</label>
              <code>{props.pair!.local_root}</code>
            </div>
            <div class="form-field readonly-field">
              <label>Drive Folder</label>
              <code>{props.pair!.remote_root_path}</code>
            </div>
          </Show>

          <div class="form-field">
            <label>Sync Mode</label>
            <select
              value={mode()}
              onChange={(e) => setMode(e.currentTarget.value)}
            >
              <For each={SYNC_MODES}>
                {(m) => <option value={m.value}>{m.label}</option>}
              </For>
            </select>
          </div>

          <div class="form-field">
            <label>Conflict Policy</label>
            <select
              value={conflictPolicy()}
              onChange={(e) => setConflictPolicy(e.currentTarget.value)}
            >
              <For each={CONFLICT_POLICIES}>
                {(p) => <option value={p.value}>{p.label}</option>}
              </For>
            </select>
          </div>

          <div class="form-field">
            <label>Poll Interval (seconds)</label>
            <input
              type="number"
              min="5"
              step="5"
              value={pollInterval()}
              onInput={(e) =>
                setPollInterval(parseInt(e.currentTarget.value, 10) || 0)
              }
            />
            <span class="form-hint">
              How often to check Drive for remote changes.
            </span>
          </div>

          <Show when={!editing()}>
            <div class="form-field">
              <label>
                <input
                  type="checkbox"
                  checked={encryptionEnabled()}
                  onChange={(e) => setEncryptionEnabled(e.currentTarget.checked)}
                />{" "}
                Encrypt files before uploading to Google Drive
              </label>
              <span class="form-hint">
                Files are encrypted with AES-256-GCM using a key derived from
                your passphrase (Argon2id). The key is stored in the OS
                keyring. Encryption can't be toggled on or off after the pair
                is created.
              </span>
            </div>
            <Show when={encryptionEnabled()}>
              <div class="form-field">
                <label>Passphrase</label>
                <input
                  type="password"
                  autocomplete="new-password"
                  value={passphrase()}
                  onInput={(e) => setPassphrase(e.currentTarget.value)}
                />
                <span class="form-hint">
                  At least 8 characters. <strong>Lose this and your
                  encrypted files become unreadable</strong> — there is no
                  recovery.
                </span>
              </div>
              <div class="form-field">
                <label>Confirm Passphrase</label>
                <input
                  type="password"
                  autocomplete="new-password"
                  value={passphraseConfirm()}
                  onInput={(e) => setPassphraseConfirm(e.currentTarget.value)}
                />
              </div>
            </Show>
          </Show>

          <Show when={editing() && props.pair?.encryption_enabled}>
            <div class="form-field readonly-field">
              <label>Encryption</label>
              <code>Enabled (AES-256-GCM)</code>
              <span class="form-hint">
                If you sync this pair on a different machine, you'll be
                prompted for the passphrase to unlock it once. The
                passphrase itself can't be changed in the UI yet.
              </span>
            </div>
          </Show>
        </div>

        <div class="modal-footer">
          <button class="btn btn-ghost" onClick={props.onClose}>
            Cancel
          </button>
          <button
            class="btn btn-primary"
            disabled={saving()}
            onClick={handleSave}
          >
            {saving() ? "Saving…" : editing() ? "Save Changes" : "Create"}
          </button>
        </div>

        <Show when={showRemotePicker()}>
          <DriveFolderPicker
            accountId={accountId()}
            onCancel={() => setShowRemotePicker(false)}
            onSelect={(id, path) => {
              setRemoteRootId(id);
              setRemoteRootPath(path);
              setShowRemotePicker(false);
            }}
          />
        </Show>
        <Show when={showLocalPicker()}>
          <LocalFolderPicker
            initialPath={localRoot()}
            onCancel={() => setShowLocalPicker(false)}
            onSelect={(path) => {
              setLocalRoot(path);
              if (!name()) {
                const base = path.split("/").filter(Boolean).pop();
                if (base) setName(base);
              }
              setShowLocalPicker(false);
            }}
          />
        </Show>
      </div>
    </div>
  );
}

// ── Local Folder Picker ───────────────────────────────────────────

function LocalFolderPicker(props: {
  initialPath: string;
  onSelect: (path: string) => void;
  onCancel: () => void;
}) {
  const [currentPath, setCurrentPath] = createSignal(props.initialPath || "");
  const [newFolderName, setNewFolderName] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  onMount(async () => {
    if (!currentPath()) {
      try {
        setCurrentPath(await invoke<string>("default_local_folder"));
      } catch (e) {
        setError(String(e));
        setCurrentPath("/");
      }
    }
  });

  const [folders, { refetch }] = createResource(currentPath, async (path) => {
    try {
      setError(null);
      return await invoke<LocalFolder[]>("list_local_folders", { path });
    } catch (e) {
      setError(String(e));
      return [];
    }
  });

  async function goUp() {
    try {
      const parent = await invoke<string | null>("parent_local_folder", {
        path: currentPath(),
      });
      if (parent) setCurrentPath(parent);
    } catch (e) {
      setError(String(e));
    }
  }

  async function createFolder() {
    const folderName = newFolderName().trim();
    if (!folderName) return;
    setCreating(true);
    try {
      const folder = await invoke<LocalFolder>("create_local_folder", {
        parentPath: currentPath(),
        name: folderName,
      });
      setNewFolderName("");
      await refetch();
      setCurrentPath(folder.path);
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div class="modal-backdrop nested" onClick={props.onCancel}>
      <div class="modal modal-sm" onClick={(e) => e.stopPropagation()}>
        <div class="modal-header">
          <h2>Pick Local Folder</h2>
          <button class="btn btn-sm btn-ghost" onClick={props.onCancel}>
            ✕
          </button>
        </div>

        <div class="breadcrumbs">
          <button class="crumb" onClick={goUp}>
            Up
          </button>
          <span class="crumb-sep">/</span>
          <span class="crumb current-path">{currentPath()}</span>
        </div>

        <div class="modal-body picker-body">
          <Show when={error()}>
            <p class="error-msg">{error()}</p>
          </Show>
          <div class="create-folder-row">
            <input
              type="text"
              value={newFolderName()}
              placeholder="New folder name"
              onInput={(e) => setNewFolderName(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") createFolder();
              }}
            />
            <button
              class="btn btn-sm"
              disabled={creating() || !newFolderName().trim()}
              onClick={createFolder}
            >
              Create
            </button>
          </div>
          <Show
            when={!folders.loading}
            fallback={<p class="empty">Loading…</p>}
          >
            <Show
              when={(folders() ?? []).length > 0}
              fallback={
                <p class="empty">No subfolders here. Select this folder?</p>
              }
            >
              <div class="picker-list">
                <For each={folders()}>
                  {(f) => (
                    <button
                      class="picker-item"
                      onClick={() => setCurrentPath(f.path)}
                    >
                      <span class="picker-icon">📁</span>
                      <span>{f.name}</span>
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </Show>
        </div>

        <div class="modal-footer">
          <span class="picker-hint">
            Selecting: <code>{currentPath()}</code>
          </span>
          <button class="btn btn-ghost" onClick={props.onCancel}>
            Cancel
          </button>
          <button class="btn btn-primary" onClick={() => props.onSelect(currentPath())}>
            Select This Folder
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Drive Folder Picker ───────────────────────────────────────────

function DriveFolderPicker(props: {
  accountId: string;
  onSelect: (id: string, path: string) => void;
  onCancel: () => void;
}) {
  // Breadcrumb stack: [{id, name}]. Root is {id: "root", name: "My Drive"}.
  const [stack, setStack] = createSignal<{ id: string; name: string }[]>([
    { id: "root", name: "My Drive" },
  ]);
  const [newFolderName, setNewFolderName] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const current = () => stack()[stack().length - 1];
  const pathString = () =>
    "/" + stack().slice(1).map((s) => s.name).join("/");

  const [folders, { refetch }] = createResource(current, async (c) => {
    try {
      setError(null);
      return await invoke<DriveFolder[]>("list_drive_folders", {
        accountId: props.accountId,
        parentId: c.id,
      });
    } catch (e) {
      setError(String(e));
      return [];
    }
  });

  function enter(folder: DriveFolder) {
    setStack([...stack(), { id: folder.id, name: folder.name }]);
  }

  function goTo(index: number) {
    setStack(stack().slice(0, index + 1));
  }

  function selectCurrent() {
    const c = current();
    const path = stack().length === 1 ? "/" : pathString();
    props.onSelect(c.id, path);
  }

  async function createFolder() {
    const folderName = newFolderName().trim();
    if (!folderName) return;
    setCreating(true);
    try {
      const folder = await invoke<DriveFolder>("create_drive_folder", {
        accountId: props.accountId,
        parentId: current().id,
        name: folderName,
      });
      setNewFolderName("");
      await refetch();
      setStack([...stack(), { id: folder.id, name: folder.name }]);
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div class="modal-backdrop nested" onClick={props.onCancel}>
      <div class="modal modal-sm" onClick={(e) => e.stopPropagation()}>
        <div class="modal-header">
          <h2>Pick Drive Folder</h2>
          <button class="btn btn-sm btn-ghost" onClick={props.onCancel}>
            ✕
          </button>
        </div>

        <div class="breadcrumbs">
          <For each={stack()}>
            {(crumb, i) => (
              <>
                <Show when={i() > 0}>
                  <span class="crumb-sep">/</span>
                </Show>
                <button
                  class="crumb"
                  disabled={i() === stack().length - 1}
                  onClick={() => goTo(i())}
                >
                  {crumb.name}
                </button>
              </>
            )}
          </For>
        </div>

        <div class="modal-body picker-body">
          <Show when={error()}>
            <p class="error-msg">{error()}</p>
          </Show>
          <div class="create-folder-row">
            <input
              type="text"
              value={newFolderName()}
              placeholder="New folder name"
              onInput={(e) => setNewFolderName(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") createFolder();
              }}
            />
            <button
              class="btn btn-sm"
              disabled={creating() || !newFolderName().trim()}
              onClick={createFolder}
            >
              Create
            </button>
          </div>
          <Show
            when={!folders.loading}
            fallback={<p class="empty">Loading…</p>}
          >
            <Show
              when={(folders() ?? []).length > 0}
              fallback={
                <p class="empty">No subfolders here. Select this folder?</p>
              }
            >
              <div class="picker-list">
                <For each={folders()}>
                  {(f) => (
                    <button class="picker-item" onClick={() => enter(f)}>
                      <span class="picker-icon">📁</span>
                      <span>{f.name}</span>
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </Show>
        </div>

        <div class="modal-footer">
          <span class="picker-hint">
            Selecting: <code>{pathString() || "/"}</code>
          </span>
          <button class="btn btn-ghost" onClick={props.onCancel}>
            Cancel
          </button>
          <button class="btn btn-primary" onClick={selectCurrent}>
            Select This Folder
          </button>
        </div>
      </div>
    </div>
  );
}

function ActivityFeed(props: {
  syncPairs: SyncPair[];
  selectedPair: string | null;
  uploads: UploadsByPair;
}) {
  const pairId = () => props.selectedPair ?? props.syncPairs[0]?.id;

  const [activity, { refetch }] = createResource(pairId, async (id) => {
    if (!id) return [];
    return await invoke<ChangeLogEntry[]>("get_recent_activity", {
      syncPairId: id,
      limit: 50,
    });
  });

  // Refresh the feed when a sync ends so finished transfers (and their
  // speeds) appear without switching tabs.
  onMount(() => {
    const unlisten = listen<string>("sync-finished", () => refetch());
    onCleanup(() => unlisten.then((u) => u()));
  });

  // Live throughput: sum the per-file speeds of everything currently in
  // flight for this pair, split by direction. Entries linger in the store
  // until the sync finishes, so drop ones that stopped reporting — a
  // finished file would otherwise keep inflating the current rate.
  const STALE_MS = 3000;
  const liveSpeed = (kind: TransferKind) => {
    const id = pairId();
    if (!id) return 0;
    const now = Date.now();
    return Object.values(props.uploads[id] ?? {})
      .filter(
        (t) =>
          t.kind === kind &&
          t.speedBps != null &&
          t.bytes < t.total &&
          now - t.lastUpdatedAt < STALE_MS,
      )
      .reduce((sum, t) => sum + (t.speedBps ?? 0), 0);
  };
  const activeCount = (kind: TransferKind) => {
    const id = pairId();
    if (!id) return 0;
    const now = Date.now();
    return Object.values(props.uploads[id] ?? {}).filter(
      (t) => t.kind === kind && t.bytes < t.total && now - t.lastUpdatedAt < STALE_MS,
    ).length;
  };
  const anyActive = () => activeCount("upload") + activeCount("download") > 0;

  const actionIcon = (action: string) => {
    switch (action) {
      case "upload": return "^";
      case "download": return "v";
      case "delete-local":
      case "delete-remote": return "x";
      case "create-local-dir":
      case "create-remote-dir": return "+";
      case "conflict": return "!";
      case "resolve": return "*";
      case "error": return "!";
      default: return "-";
    }
  };

  return (
    <div class="activity">
      <div class="activity-head">
        <h2>Recent Activity</h2>
        <div class="speed-readout" classList={{ idle: !anyActive() }}>
          <div class="speed-item">
            <span class="speed-arrow up">↑</span>
            <span class="speed-value">{formatSpeed(liveSpeed("upload"))}</span>
            <span class="speed-label">
              {activeCount("upload") > 0 ? `${activeCount("upload")} uploading` : "idle"}
            </span>
          </div>
          <div class="speed-item">
            <span class="speed-arrow down">↓</span>
            <span class="speed-value">{formatSpeed(liveSpeed("download"))}</span>
            <span class="speed-label">
              {activeCount("download") > 0
                ? `${activeCount("download")} downloading`
                : "idle"}
            </span>
          </div>
        </div>
      </div>
      <Show
        when={(activity() ?? []).length > 0}
        fallback={<p class="empty">No activity yet. Run a sync to see events here.</p>}
      >
        <div class="activity-list">
          <For each={activity()}>
            {(entry) => (
              <div class={`activity-item action-${entry.action}`}>
                <span class="activity-icon">{actionIcon(entry.action)}</span>
                <div class="activity-detail">
                  <div class="activity-path">{entry.relative_path}</div>
                  <div class="activity-meta">
                    {actionLabel(entry)}
                    {" · "}
                    {new Date(entry.created_at).toLocaleString()}
                  </div>
                </div>
                <Show when={entry.bytes != null}>
                  <div class="activity-transfer">
                    <span class="activity-size">{formatBytes(entry.bytes ?? 0)}</span>
                    <Show when={entrySpeed(entry) != null}>
                      <span class="activity-speed">
                        {formatSpeed(entrySpeed(entry) ?? 0)}
                      </span>
                    </Show>
                  </div>
                </Show>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

/// What one change-log row says it did, in words rather than the raw
/// action string. Folder deletes carry detail="folder" from the engine,
/// which is the only way to tell them apart after the path is gone.
function actionLabel(entry: ChangeLogEntry): string {
  const folder = entry.detail === "folder";
  switch (entry.action) {
    case "upload": return "uploaded";
    case "download": return "downloaded";
    case "delete-local":
      return folder ? "removed local folder" : "removed locally";
    case "delete-remote":
      return folder ? "removed folder from Drive" : "removed from Drive";
    case "create-local-dir": return "created local folder";
    case "create-remote-dir": return "created folder on Drive";
    case "conflict": return "conflict";
    case "resolve": return "conflict resolved";
    case "error": return entry.detail ? `error - ${entry.detail}` : "error";
    default:
      return entry.detail ? `${entry.action} - ${entry.detail}` : entry.action;
  }
}

/// The in-card activity block: what the sync is doing right now, how far
/// through it is, and how fast bytes are moving.
function SyncActivity(props: {
  status: SyncStatus | undefined;
  transfers: UploadEntry[];
}) {
  const STALE_MS = 3000;
  const live = () =>
    props.transfers.filter(
      (t) => t.bytes < t.total && Date.now() - t.lastUpdatedAt < STALE_MS,
    );
  const speed = (kind: TransferKind) =>
    live()
      .filter((t) => t.kind === kind)
      .reduce((sum, t) => sum + (t.speedBps ?? 0), 0);

  const phase = () => props.status?.phase ?? "scanning-local";
  const label = () => PHASE_LABEL[phase()];
  const total = () => props.status?.total ?? 0;
  const done = () => props.status?.done ?? 0;
  // Only "executing" has a meaningful fraction; the earlier phases have no
  // countable unit of work, so they get an indeterminate bar rather than a
  // fake percentage.
  const determinate = () => phase() === "executing" && total() > 0;
  const pct = () => (determinate() ? Math.round((done() / total()) * 100) : 0);

  return (
    <div class="sync-activity">
      <div class="sync-activity-head">
        <span class="sync-activity-phase">{label()}</span>
        <Show when={determinate()}>
          <span class="sync-activity-count">
            {done()} / {total()}
          </span>
        </Show>
      </div>
      <div class="sync-activity-bar" classList={{ indeterminate: !determinate() }}>
        <div
          class="sync-activity-fill"
          style={determinate() ? { width: `${pct()}%` } : undefined}
        />
      </div>
      <div class="sync-activity-meta">
        <span class="sync-activity-path">
          {props.status?.path ?? "…"}
        </span>
        <span class="sync-activity-speed">
          <Show when={speed("upload") > 0}>↑ {formatSpeed(speed("upload"))} </Show>
          <Show when={speed("download") > 0}>↓ {formatSpeed(speed("download"))}</Show>
        </span>
      </div>
    </div>
  );
}

/// Average speed of one completed transfer. Null when the row predates byte
/// accounting, or when it isn't a transfer at all.
function entrySpeed(entry: ChangeLogEntry): number | null {
  if (entry.bytes == null || entry.duration_ms == null || entry.duration_ms <= 0) {
    return null;
  }
  return (entry.bytes * 1000) / entry.duration_ms;
}

function formatSpeed(bytesPerSecond: number): string {
  if (!bytesPerSecond || bytesPerSecond <= 0) return "—";
  return `${formatBytes(Math.round(bytesPerSecond))}/s`;
}

function StatisticsView(props: { syncPairs: SyncPair[] }) {
  const [scope, setScope] = createSignal<string>("all");

  const scopeArg = () => (scope() === "all" ? undefined : scope());

  const [stats, { refetch }] = createResource(scope, async () => {
    return await invoke<StatsPayload>("get_transfer_stats", {
      syncPairId: scopeArg() ?? null,
    });
  });

  const [byPair] = createResource(async () => {
    return await invoke<PairStats[]>("get_transfer_stats_by_pair");
  });

  onMount(() => {
    const unlisten = listen<string>("sync-finished", () => refetch());
    onCleanup(() => unlisten.then((u) => u()));
  });

  const totalBytes = (s: TransferStats | undefined) =>
    (s?.uploaded.bytes ?? 0) + (s?.downloaded.bytes ?? 0);
  const totalFiles = (s: TransferStats | undefined) =>
    (s?.uploaded.files ?? 0) + (s?.downloaded.files ?? 0);

  const avgSpeed = (d: DirectionTotals | undefined) => {
    if (!d || d.duration_ms <= 0 || d.bytes <= 0) return null;
    return (d.bytes * 1000) / d.duration_ms;
  };

  // Rows written before schema v3 have no byte measurement. Say so rather
  // than letting a user read a total that silently excludes them.
  const unmeasured = (s: TransferStats | undefined) =>
    ((s?.uploaded.files ?? 0) - (s?.uploaded.measured_files ?? 0)) +
    ((s?.downloaded.files ?? 0) - (s?.downloaded.measured_files ?? 0));

  return (
    <div class="statistics">
      <div class="activity-head">
        <h2>Statistics</h2>
        <select
          class="input input-sm"
          value={scope()}
          onChange={(e) => setScope(e.currentTarget.value)}
        >
          <option value="all">All sync pairs</option>
          <For each={props.syncPairs}>
            {(p) => <option value={p.id}>{p.name}</option>}
          </For>
        </select>
      </div>

      <Show
        when={stats() && totalFiles(stats()?.allTime) > 0}
        fallback={
          <p class="empty">
            Nothing transferred yet. Run a sync and the totals will show up here.
          </p>
        }
      >
        <div class="stat-grid">
          <StatCard
            title="Uploaded"
            arrow="↑"
            files={stats()!.allTime.uploaded.files}
            bytes={stats()!.allTime.uploaded.bytes}
            speed={avgSpeed(stats()!.allTime.uploaded)}
          />
          <StatCard
            title="Downloaded"
            arrow="↓"
            files={stats()!.allTime.downloaded.files}
            bytes={stats()!.allTime.downloaded.bytes}
            speed={avgSpeed(stats()!.allTime.downloaded)}
          />
        </div>

        <div class="stat-strip">
          <div class="stat-chip">
            <span class="stat-chip-value">{totalFiles(stats()?.allTime)}</span>
            <span class="stat-chip-label">transfers</span>
          </div>
          <div class="stat-chip">
            <span class="stat-chip-value">{formatBytes(totalBytes(stats()?.allTime))}</span>
            <span class="stat-chip-label">moved</span>
          </div>
          <div class="stat-chip">
            <span class="stat-chip-value">{stats()!.allTime.deleted}</span>
            <span class="stat-chip-label">deletes</span>
          </div>
          <div class="stat-chip">
            <span class="stat-chip-value">{stats()!.allTime.conflicts}</span>
            <span class="stat-chip-label">conflicts</span>
          </div>
          <div class="stat-chip">
            <span class="stat-chip-value">{stats()!.allTime.errors}</span>
            <span class="stat-chip-label">errors</span>
          </div>
        </div>

        <h3 class="stat-section">Last 7 days</h3>
        <div class="stat-grid">
          <StatCard
            title="Uploaded"
            arrow="↑"
            files={stats()!.last7Days.uploaded.files}
            bytes={stats()!.last7Days.uploaded.bytes}
            speed={avgSpeed(stats()!.last7Days.uploaded)}
          />
          <StatCard
            title="Downloaded"
            arrow="↓"
            files={stats()!.last7Days.downloaded.files}
            bytes={stats()!.last7Days.downloaded.bytes}
            speed={avgSpeed(stats()!.last7Days.downloaded)}
          />
        </div>

        <Show when={scope() === "all" && (byPair() ?? []).length > 1}>
          <h3 class="stat-section">By sync pair</h3>
          <table class="stat-table">
            <thead>
              <tr>
                <th>Sync pair</th>
                <th>Uploaded</th>
                <th>Downloaded</th>
                <th>Total</th>
              </tr>
            </thead>
            <tbody>
              <For each={byPair()}>
                {(row) => (
                  <tr>
                    <td>{row.name}</td>
                    <td>
                      {row.stats.uploaded.files} · {formatBytes(row.stats.uploaded.bytes)}
                    </td>
                    <td>
                      {row.stats.downloaded.files} ·{" "}
                      {formatBytes(row.stats.downloaded.bytes)}
                    </td>
                    <td>{formatBytes(totalBytes(row.stats))}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>

        <p class="stat-footnote">
          <Show when={stats()!.allTime.since}>
            Counting since{" "}
            {new Date(stats()!.allTime.since as string).toLocaleString()}.{" "}
          </Show>
          <Show when={unmeasured(stats()?.allTime) > 0}>
            {unmeasured(stats()?.allTime)} older transfer
            {unmeasured(stats()?.allTime) === 1 ? "" : "s"} predate byte
            accounting and count toward the file totals but not the byte totals.
          </Show>
        </p>
      </Show>
    </div>
  );
}

function StatCard(props: {
  title: string;
  arrow: string;
  files: number;
  bytes: number;
  speed: number | null;
}) {
  return (
    <div class="stat-card">
      <div class="stat-card-head">
        <span class="stat-arrow">{props.arrow}</span>
        <span class="stat-card-title">{props.title}</span>
      </div>
      <div class="stat-card-value">{formatBytes(props.bytes)}</div>
      <div class="stat-card-meta">
        {props.files} file{props.files === 1 ? "" : "s"}
        {" · avg "}
        {props.speed != null ? formatSpeed(props.speed) : "—"}
      </div>
    </div>
  );
}

function ConflictsView(props: { syncPairs: SyncPair[]; selectedPair: string | null }) {
  const pairId = () => props.selectedPair ?? props.syncPairs[0]?.id;
  const [resolving, setResolving] = createSignal<string | null>(null);

  const [conflicts, { refetch }] = createResource(pairId, async (id) => {
    if (!id) return [];
    return await invoke<FileEntry[]>("get_conflicts", { syncPairId: id });
  });

  async function handleResolve(
    syncPairId: string,
    relativePath: string,
    resolution: string
  ) {
    setResolving(relativePath);
    try {
      await invoke("resolve_conflict", {
        syncPairId,
        relativePath,
        resolution,
      });
      refetch();
    } catch (e) {
      console.error("Resolution failed:", e);
    } finally {
      setResolving(null);
    }
  }

  return (
    <div class="conflicts">
      <h2>Conflicts</h2>
      <Show
        when={(conflicts() ?? []).length > 0}
        fallback={<p class="empty">No conflicts. Everything is in sync.</p>}
      >
        <div class="card-list">
          <For each={conflicts()}>
            {(entry) => (
              <div class="card card-conflict">
                <div class="card-title">{entry.relative_path}</div>
                <div class="card-body">
                  <p>Both local and remote versions have changed.</p>
                </div>
                <div class="card-footer">
                  <button
                    class="btn btn-sm"
                    disabled={resolving() === entry.relative_path}
                    onClick={() =>
                      handleResolve(entry.sync_pair_id, entry.relative_path, "keep-local")
                    }
                  >
                    Keep Local
                  </button>
                  <button
                    class="btn btn-sm"
                    disabled={resolving() === entry.relative_path}
                    onClick={() =>
                      handleResolve(entry.sync_pair_id, entry.relative_path, "keep-remote")
                    }
                  >
                    Keep Remote
                  </button>
                  <button
                    class="btn btn-sm"
                    disabled={resolving() === entry.relative_path}
                    onClick={() =>
                      handleResolve(entry.sync_pair_id, entry.relative_path, "keep-both")
                    }
                  >
                    Keep Both
                  </button>
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

export default App;
