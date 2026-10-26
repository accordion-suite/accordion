"use strict";

const $ = (id) => document.getElementById(id);

const PROBLEM = "Accordion ran into a problem. Try again in a moment.";

// How often the page checks again while a backup or test runs.
const RUNNING_MS = 3 * 1000;

// When adding a location gets a second, longer note. A location that
// doesn't answer has already failed by then, after 10 seconds.
const STILL_WORKING_MS = 15 * 1000;

let status = null;
// What to do once the admin confirms their password.
let afterConfirm = null;

function showError(message) {
  $("error").textContent = message;
  $("error").hidden = !message;
}

// Returns the response's status with its data, so callers can tell "not
// signed in" from other problems.
async function api(method, path, body) {
  let response;
  try {
    response = await fetch(path, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body && JSON.stringify(body),
    });
  } catch {
    throw new Error("Accordion didn't respond. Check that the server is on, then try again.");
  }
  const data = await response.json().catch(() => ({}));
  return { status: response.status, data };
}

// Such as "October 22, 2026 at 3:04 AM", in the reader's time zone.
function when(millis) {
  return new Date(millis).toLocaleString(undefined, { dateStyle: "long", timeStyle: "short" });
}

// Such as "12.4 GB".
function size(bytes) {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1e3))} KB`;
}

function hourLabel(hour) {
  if (hour === 0) return "midnight";
  if (hour === 12) return "noon";
  return hour < 12 ? `${hour} a.m.` : `${hour - 12} p.m.`;
}

function show(id) {
  for (const section of ["sign-in", "confirm", "turn-on", "key", "unconfirmed", "manage"]) {
    $(section).hidden = section !== id && !(id === "unconfirmed" && section === "manage");
  }
}

function describeTarget(target) {
  const lines = [];
  const backup = target.last_backup;
  if (!backup) {
    lines.push(`No backups yet. The first one starts at ${hourLabel(status.hour)}.`);
  } else if (backup.ok) {
    lines.push(`Last backed up ${when(backup.finished_at)}.`);
  } else {
    let line = `The last backup, ${when(backup.finished_at)}, didn't work. ${backup.problem}`;
    if (target.last_good_backup) {
      line += ` The last one that worked was ${when(target.last_good_backup)}.`;
    }
    lines.push(line);
  }
  if (target.last_check && !target.last_check.ok) {
    lines.push(`The last check found a problem. ${target.last_check.problem}`);
  }
  if (target.append_only) {
    lines.push(
      "This server keeps every backup, so someone who takes over this server can't delete them.",
    );
    if (target.stored_bytes != null && target.oldest_backup != null) {
      lines.push(
        `Uses ${size(target.stored_bytes)}, with backups back to ${when(target.oldest_backup)}. Accordion never removes old backups here, so check its free space now and then.`,
      );
    }
  } else if (target.kind === "rest") {
    lines.push(
      "This server lets Accordion delete backups, so someone who takes over this server could delete them too. Turn on append-only mode in the REST server to protect them.",
    );
  }
  const test = target.last_restore_test;
  if (!test) {
    lines.push("No restore test yet. Accordion runs one each month.");
  } else if (test.ok) {
    lines.push(`Restore test passed ${when(test.finished_at)}.`);
  } else {
    lines.push(`The restore test ${when(test.finished_at)} didn't pass. ${test.problem}`);
  }
  return lines;
}

function renderTargets() {
  const list = $("targets");
  list.replaceChildren();
  $("no-targets").hidden = status.targets.length > 0;
  for (const target of status.targets) {
    const item = document.createElement("li");
    const name = document.createElement("strong");
    name.textContent = target.name;
    const location = document.createElement("p");
    location.className = "hint";
    location.textContent = target.location;
    item.append(name, location);
    for (const line of describeTarget(target)) {
      const text = document.createElement("p");
      text.textContent = line;
      item.append(text);
    }
    const test = document.createElement("button");
    test.type = "button";
    test.className = "secondary";
    test.textContent = "Test restore";
    test.disabled = status.running || !target.last_good_backup;
    test.addEventListener("click", () => act(test, "POST", `/api/backups/targets/${target.id}/test`));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "secondary";
    remove.textContent = "Remove";
    remove.addEventListener("click", () => removeTarget(target, remove));
    item.append(test, " ", remove);
    list.append(item);
  }
}

function render() {
  if (status.key === "none") {
    show("turn-on");
    return;
  }
  if ($("key").hidden || status.key === "confirmed") {
    show(status.key === "unconfirmed" ? "unconfirmed" : "manage");
  }
  let summary;
  if (status.running) {
    summary = "A backup or test is running. This page updates when it's done.";
  } else if (status.targets.length === 0) {
    summary = "Backups are on, but there's nowhere for them to go yet. Add a location below.";
  } else {
    summary = `Accordion backs up to every location below each night at ${hourLabel(status.hour)}.`;
  }
  $("summary").textContent = summary;
  $("run").disabled = status.running || status.targets.length === 0;
  if ($("hour").value !== String(status.hour)) $("hour").value = String(status.hour);
  renderTargets();
}

async function load() {
  try {
    const { status: code, data } = await api("GET", "/api/backups");
    if (code === 401) {
      show("sign-in");
      return;
    }
    if (code !== 200) throw new Error(data.error || PROBLEM);
    status = data;
    render();
    if (status.running) setTimeout(load, RUNNING_MS);
  } catch (error) {
    showError(error.message);
  }
}

function showKey(grouped) {
  $("key-letters").textContent = grouped;
  $("typed-key").value = "";
  show("key");
}

// Gets the recovery key with `method`: POST creates it, GET shows it again.
// Asks for the password first if the sign-in isn't recent.
async function fetchKey(method) {
  const { status: code, data } = await api(method, "/api/backups/key");
  if (code === 403 && !data.error?.startsWith("Only admins")) {
    afterConfirm = () => fetchKey(method);
    show("confirm");
    $("confirm-password").focus();
    return;
  }
  if (code !== 200 && code !== 201) throw new Error(data.error || PROBLEM);
  showKey(data.recovery_key);
}

async function act(button, method, path, body) {
  showError("");
  button.disabled = true;
  try {
    const { status: code, data } = await api(method, path, body);
    if (code >= 300) throw new Error(data.error || PROBLEM);
    await load();
  } catch (error) {
    showError(error.message);
    button.disabled = false;
  }
}

async function removeTarget(target, button) {
  const sure = confirm(
    `Stop backing up to ${target.name}? The backups already there stay, so you can still restore them.`,
  );
  if (sure) await act(button, "DELETE", `/api/backups/targets/${target.id}`);
}

// Shows what's happening in the line `id` while a slow request runs, and
// switches to `later` if it takes a while. Returns a function that hides it.
function whileWaiting(id, first, later) {
  const line = $(id);
  line.textContent = first;
  line.hidden = false;
  const timer = setTimeout(() => {
    line.textContent = later;
  }, STILL_WORKING_MS);
  return () => {
    clearTimeout(timer);
    line.hidden = true;
    line.textContent = "";
  };
}

function onSubmit(id, action) {
  $(id).addEventListener("submit", async (event) => {
    event.preventDefault();
    showError("");
    const button = event.submitter;
    button.disabled = true;
    try {
      await action();
    } catch (error) {
      showError(error.message);
    } finally {
      button.disabled = false;
    }
  });
}

onSubmit("sign-in", async () => {
  const { status: code, data } = await api("POST", "/api/session", {
    username: $("username").value,
    password: $("password").value,
  });
  if (code !== 200) throw new Error(data.error || PROBLEM);
  $("password").value = "";
  await load();
});

onSubmit("confirm", async () => {
  const { status: code, data } = await api("POST", "/api/session/confirm", {
    password: $("confirm-password").value,
  });
  if (code !== 204) throw new Error(data.error || PROBLEM);
  $("confirm-password").value = "";
  const next = afterConfirm;
  afterConfirm = null;
  if (next) await next();
});

onSubmit("key-confirm", async () => {
  const { status: code, data } = await api("POST", "/api/backups/key/confirm", {
    recovery_key: $("typed-key").value,
  });
  if (code !== 204) throw new Error(data.error || PROBLEM);
  $("key-letters").textContent = "";
  $("key").hidden = true;
  await load();
});

// Shows the fields for the chosen kind of location. Turning the others off
// keeps their empty, required fields from blocking the form.
for (const radio of document.querySelectorAll("input[name=target-kind]")) {
  radio.addEventListener("change", async () => {
    for (const kind of ["folder", "s3", "sftp", "rest"]) {
      const fields = $(`target-${kind}`);
      fields.hidden = fields.disabled = kind !== radio.value;
    }
    if (radio.value === "sftp" && !$("target-ssh-key").textContent) {
      showError("");
      try {
        const { status: code, data } = await api("GET", "/api/backups/ssh-key");
        if (code !== 200) throw new Error(data.error || PROBLEM);
        $("target-ssh-key").textContent = data.public_key;
      } catch (error) {
        showError(error.message);
      }
    }
  });
}

$("target-copy-key").addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText($("target-ssh-key").textContent);
    $("target-copy-key").textContent = "Copied";
  } catch {
    // Clipboard access needs HTTPS. Select the key so it's one copy away.
    const range = document.createRange();
    range.selectNodeContents($("target-ssh-key"));
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    showError("Accordion couldn't copy the key, so it's selected for you to copy.");
  }
});

onSubmit("add-target", async () => {
  const kind = document.querySelector("input[name=target-kind]:checked").value;
  const place = {
    folder: { kind, path: $("target-path").value },
    s3: {
      kind,
      endpoint: $("target-endpoint").value,
      bucket: $("target-bucket").value,
      key_id: $("target-key-id").value,
      secret_key: $("target-secret").value,
    },
    sftp: {
      kind,
      server: $("target-server").value,
      username: $("target-username").value,
      folder: $("target-remote-folder").value,
    },
    rest: {
      kind,
      address: $("target-address").value,
      username: $("target-rest-username").value,
      password: $("target-rest-password").value,
    },
  }[kind];
  const done = whileWaiting(
    "add-progress",
    kind === "folder"
      ? "Setting up the folder for backups."
      : "Connecting to the location and setting it up for backups. This usually takes a few seconds.",
    "Still setting it up. Over a slow connection, this can take a minute or two.",
  );
  let response;
  try {
    response = await api("POST", "/api/backups/targets", {
      name: $("target-name").value,
      ...place,
    });
  } finally {
    done();
  }
  const { status: code, data } = response;
  if (code !== 201) throw new Error(data.error || PROBLEM);
  for (const id of [
    "target-name",
    "target-path",
    "target-endpoint",
    "target-bucket",
    "target-key-id",
    "target-secret",
    "target-server",
    "target-username",
    "target-remote-folder",
    "target-address",
    "target-rest-username",
    "target-rest-password",
  ]) {
    $(id).value = "";
  }
  await load();
});

for (const [id, method] of [
  ["turn-on-button", "POST"],
  ["show-key", "GET"],
  ["show-key-again", "GET"],
]) {
  $(id).addEventListener("click", async () => {
    showError("");
    try {
      await fetchKey(method);
    } catch (error) {
      showError(error.message);
    }
  });
}

$("print").addEventListener("click", () => window.print());
$("run").addEventListener("click", () => act($("run"), "POST", "/api/backups/run"));

for (let hour = 0; hour < 24; hour++) {
  const option = document.createElement("option");
  option.value = String(hour);
  option.textContent = hourLabel(hour);
  $("hour").append(option);
}
$("hour").addEventListener("change", async () => {
  // Unlike the buttons, nothing else turns the menu back on.
  await act($("hour"), "PUT", "/api/backups/settings", { hour: Number($("hour").value) });
  $("hour").disabled = false;
});

load();
