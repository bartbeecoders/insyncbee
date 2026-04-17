import { createSignal, createResource, Show, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

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
}

interface DriveFolder {
  id: string;
  name: string;
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
}

type Tab = "dashboard" | "activity" | "conflicts";

function App() {
  const [tab, setTab] = createSignal<Tab>("dashboard");
  const [accounts, { refetch: refetchAccounts }] = createResource(fetchAccounts);
  const [syncPairs, { refetch: refetchPairs }] = createResource(fetchSyncPairs);
  const [selectedPair, setSelectedPair] = createSignal<string | null>(null);

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
            class={tab() === "conflicts" ? "tab active" : "tab"}
            onClick={() => setTab("conflicts")}
          >
            Conflicts
          </button>
        </nav>
      </header>

      <main class="main">
        <Show when={tab() === "dashboard"}>
          <Dashboard
            accounts={accounts() ?? []}
            syncPairs={syncPairs() ?? []}
            onRefresh={() => { refetchAccounts(); refetchPairs(); }}
            onSelectPair={setSelectedPair}
          />
        </Show>
        <Show when={tab() === "activity"}>
          <ActivityFeed syncPairs={syncPairs() ?? []} selectedPair={selectedPair()} />
        </Show>
        <Show when={tab() === "conflicts"}>
          <ConflictsView
            syncPairs={syncPairs() ?? []}
            selectedPair={selectedPair()}
          />
        </Show>
      </main>
    </div>
  );
}

function Dashboard(props: {
  accounts: Account[];
  syncPairs: SyncPair[];
  onRefresh: () => void;
  onSelectPair: (id: string | null) => void;
}) {
  const [loggingIn, setLoggingIn] = createSignal(false);
  const [loginError, setLoginError] = createSignal<string | null>(null);
  const [syncing, setSyncing] = createSignal<string | null>(null);
  const [editingPair, setEditingPair] = createSignal<SyncPair | null>(null);
  const [showForm, setShowForm] = createSignal(false);
  const [deleting, setDeleting] = createSignal<string | null>(null);

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

  async function handleLogout(accountId: string) {
    try {
      await invoke("logout", { accountId });
      props.onRefresh();
    } catch (e) {
      console.error("Logout failed:", e);
    }
  }

  async function handleSync(pairId: string) {
    setSyncing(pairId);
    try {
      const report = await invoke<string>("trigger_sync", { syncPairId: pairId });
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
                    <button
                      class="btn btn-sm btn-ghost btn-danger"
                      onClick={() => handleLogout(acc.id)}
                    >
                      Remove
                    </button>
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
                  onClick={() => props.onSelectPair(pair.id)}
                >
                  <div class="card-header">
                    <span class={`status-dot status-${pair.status}`} />
                    <div class="card-title">{pair.name}</div>
                    <span class="badge">{pair.mode}</span>
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
                  </div>
                  <div class="card-footer">
                    <button
                      class="btn btn-sm"
                      disabled={syncing() === pair.id}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleSync(pair.id);
                      }}
                    >
                      {syncing() === pair.id ? "Syncing..." : "Sync Now"}
                    </button>
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
    </div>
  );
}

// ── Sync Pair Form Modal ──────────────────────────────────────────

const SYNC_MODES: { value: string; label: string }[] = [
  { value: "two-way", label: "Two-way (bidirectional)" },
  { value: "local-to-cloud", label: "Upload only (local → Drive)" },
  { value: "cloud-to-local", label: "Download only (Drive → local)" },
];

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
  const [showRemotePicker, setShowRemotePicker] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function pickLocalFolder() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select local sync folder",
      });
      if (typeof selected === "string") {
        setLocalRoot(selected);
        if (!name()) {
          const base = selected.split("/").filter(Boolean).pop();
          if (base) setName(base);
        }
      }
    } catch (e) {
      setError(`Failed to open folder picker: ${e}`);
    }
  }

  function validate(): string | null {
    if (!name().trim()) return "Name is required.";
    if (!editing()) {
      if (!accountId()) return "Select a Google account.";
      if (!localRoot().trim()) return "Pick a local folder.";
      if (!remoteRootId().trim()) return "Pick a Drive folder.";
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
                <button class="btn btn-sm" onClick={pickLocalFolder}>
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
  const [error, setError] = createSignal<string | null>(null);

  const current = () => stack()[stack().length - 1];
  const pathString = () =>
    "/" + stack().slice(1).map((s) => s.name).join("/");

  const [folders] = createResource(current, async (c) => {
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

function ActivityFeed(props: { syncPairs: SyncPair[]; selectedPair: string | null }) {
  const pairId = () => props.selectedPair ?? props.syncPairs[0]?.id;

  const [activity] = createResource(pairId, async (id) => {
    if (!id) return [];
    return await invoke<ChangeLogEntry[]>("get_recent_activity", {
      syncPairId: id,
      limit: 50,
    });
  });

  const actionIcon = (action: string) => {
    switch (action) {
      case "upload": return "^";
      case "download": return "v";
      case "delete-local":
      case "delete-remote": return "x";
      case "conflict": return "!";
      case "resolve": return "*";
      case "error": return "!";
      default: return "-";
    }
  };

  return (
    <div class="activity">
      <h2>Recent Activity</h2>
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
                    {entry.action}
                    {entry.detail ? ` - ${entry.detail}` : ""}
                    {" · "}
                    {new Date(entry.created_at).toLocaleString()}
                  </div>
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>
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
