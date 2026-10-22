"use strict";

// The claim token lasts until setup ends, so a reload keeps your place.
const TOKEN_KEY = "accordion-setup";
const $ = (id) => document.getElementById(id);

function token() {
  try { return sessionStorage.getItem(TOKEN_KEY); } catch { return null; }
}

function saveToken(value) {
  try { sessionStorage.setItem(TOKEN_KEY, value); } catch { /* the page still works until it's reloaded */ }
  memoryToken = value;
}

let memoryToken = token();

// What the admin chose after claiming: "name" for a new server, or
// "restore". Until then, the wizard asks.
let choice = null;

function show(id) {
  for (const section of ["claim", "start", "restore", "choose", "restoring", "restored", "name", "working", "done"]) {
    $(section).hidden = section !== id;
  }
}

// Such as "October 22, 2026 at 3:04 AM", in the reader's time zone.
function when(millis) {
  return new Date(millis).toLocaleString(undefined, { dateStyle: "long", timeStyle: "short" });
}

// Such as "1.2 GB".
function size(bytes) {
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return unit === 0 ? `${value} bytes` : `${value.toFixed(1)} ${units[unit]}`;
}

function showError(message) {
  $("error").textContent = message;
  $("error").hidden = !message;
}

async function api(method, path, body) {
  const headers = { "Content-Type": "application/json" };
  if (memoryToken) headers.Authorization = `Bearer ${memoryToken}`;
  let response;
  try {
    response = await fetch(path, { method, headers, body: body && JSON.stringify(body) });
  } catch {
    throw new Error("Accordion didn't respond. Check that the server is on, then try again.");
  }
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data.error || "Accordion ran into a problem. Try again in a moment.");
  }
  return data;
}

function showProgress(progress) {
  switch (progress.state) {
    case "working":
      $("working-name").textContent = progress.name;
      show("working");
      setTimeout(refresh, 3000);
      break;
    case "done":
      $("done-link").href = progress.url;
      show("done");
      // Give the name's new DNS record a moment, then move on.
      setTimeout(() => { window.location.href = progress.url; }, 2000);
      break;
    case "failed":
      showError(progress.error);
      show("name");
      break;
    default:
      show(choice || "start");
  }
}

function showRestore(restore) {
  switch (restore.state) {
    case "working":
      $("restore-progress").value = restore.percent;
      $("restore-percent").textContent = `${restore.percent}% done`;
      show("restoring");
      setTimeout(refresh, 2000);
      break;
    case "done":
      $("restored-named").hidden = !restore.url;
      $("restored-unnamed").hidden = Boolean(restore.url);
      show("restored");
      if (restore.url) {
        $("restored-link").href = restore.url;
        // The server starts again in a few seconds. Give it time, then move on.
        setTimeout(() => { window.location.href = restore.url; }, 15000);
      }
      break;
    case "failed":
      showError(restore.error);
      show("restore");
      break;
  }
}

function showSnapshots(snapshots) {
  const list = $("snapshots");
  list.replaceChildren();
  snapshots.forEach((snapshot, index) => {
    const label = document.createElement("label");
    label.className = "choice";
    const radio = document.createElement("input");
    radio.type = "radio";
    radio.name = "snapshot";
    radio.value = snapshot.id;
    radio.checked = index === 0;
    const text = snapshot.size == null ? when(snapshot.time) : `${when(snapshot.time)}, ${size(snapshot.size)}`;
    label.append(radio, text);
    list.append(label);
  });
  show("choose");
}

async function refresh() {
  try {
    const status = await api("GET", "/api/setup");
    if (status.restore && status.restore.state !== "not_started"
        && !(status.restore.state === "failed" && choice !== "restore")) {
      showRestore(status.restore);
      return;
    }
    if (status.progress) {
      showProgress(status.progress);
      return;
    }
    $("code-field").hidden = !status.code_needed;
    $("code-help").hidden = !status.code_needed;
    $("window-help").hidden = status.code_needed;
    show("claim");
  } catch (error) {
    showError(error.message);
    setTimeout(refresh, 5000);
  }
}

$("claim").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  try {
    const { token: claimed } = await api("POST", "/api/setup/claim", { code: $("code").value });
    saveToken(claimed);
    await refresh();
  } catch (error) {
    showError(error.message);
  }
});

// Forgets the claim once it's expired, so a reload asks for the code again.
function forgetExpired(error) {
  if (error.message.startsWith("This setup page has expired")) {
    saveToken("");
  }
}

$("choose-new").addEventListener("click", () => {
  choice = "name";
  showError("");
  show("name");
});

$("choose-restore").addEventListener("click", () => {
  choice = "restore";
  showError("");
  show("restore");
});

$("name-back").addEventListener("click", () => {
  choice = null;
  showError("");
  show("start");
});

$("restore-back").addEventListener("click", () => {
  choice = null;
  showError("");
  show("start");
});

$("choose-back").addEventListener("click", () => {
  showError("");
  show("restore");
});

$("restore").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  const button = event.submitter;
  button.disabled = true;
  try {
    const { snapshots } = await api("POST", "/api/setup/restore/find", {
      path: $("restore-path").value,
      recovery_key: $("restore-key").value,
    });
    showSnapshots(snapshots);
  } catch (error) {
    forgetExpired(error);
    showError(error.message);
  } finally {
    button.disabled = false;
  }
});

$("choose").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  const chosen = document.querySelector("input[name=snapshot]:checked");
  if (!chosen) {
    showError("Choose a backup to restore.");
    return;
  }
  const button = event.submitter;
  button.disabled = true;
  try {
    await api("POST", "/api/setup/restore", {
      path: $("restore-path").value,
      recovery_key: $("restore-key").value,
      snapshot: chosen.value,
    });
    $("restore-key").value = "";
    await refresh();
  } catch (error) {
    forgetExpired(error);
    showError(error.message);
  } finally {
    button.disabled = false;
  }
});

$("name").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  const button = event.submitter;
  button.disabled = true;
  try {
    await api("POST", "/api/setup/name", {
      name: $("server-name").value,
      provider: $("provider").value,
      token: $("token").value,
    });
    $("token").value = "";
    await refresh();
  } catch (error) {
    forgetExpired(error);
    showError(error.message);
  } finally {
    button.disabled = false;
  }
});

refresh();
