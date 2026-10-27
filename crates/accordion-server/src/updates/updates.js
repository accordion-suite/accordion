"use strict";

const $ = (id) => document.getElementById(id);

const PROBLEM = "Accordion ran into a problem. Try again in a moment.";

// How often the page checks again while an update installs.
const INSTALLING_MS = 5 * 1000;

let status = null;

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

// Such as "October 22, 2026".
function dateOf(millis) {
  return new Date(millis).toLocaleDateString(undefined, { dateStyle: "long" });
}

// A release date such as "2026-11-02", as "November 2, 2026". Read as a
// local date, so it doesn't shift a day in time zones behind UTC.
function day(date) {
  const [year, month, dayOfMonth] = date.split("-").map(Number);
  return new Date(year, month - 1, dayOfMonth).toLocaleDateString(undefined, {
    dateStyle: "long",
  });
}

// One line about what the update helper did.
function describe(report) {
  const reason = report.reason ? ` ${report.reason}.` : "";
  if (report.outcome === "installed") {
    return `${when(report.finished_at)}: updated from ${report.from} to ${report.version}.`;
  }
  if (report.outcome === "rolled-back") {
    return `${when(report.finished_at)}: Accordion ${report.version} didn't work, so Accordion went back to ${report.from}, with the data as it was.${reason}`;
  }
  return `${when(report.finished_at)}: Accordion didn't update to ${report.version}.${reason}`;
}

function renderWaiting() {
  const newest = status.newest;
  $("waiting-title").textContent = `Accordion ${newest.version} is available`;
  $("waiting-published").textContent = `Released ${day(newest.published)}.`;
  $("waiting-security").hidden = !newest.security;
  $("waiting-notes").textContent = newest.notes;
  $("waiting-link").href = newest.link;

  $("waiting-failed").hidden = !status.failed;
  if (status.failed) {
    $("waiting-failed").textContent = `${describe(status.failed)} Accordion won't install ${newest.version} on its own, but it will install a newer release.`;
  }

  $("waiting-here").hidden = !status.can_install;
  $("waiting-command").hidden = status.can_install;
  if (!status.installs_from) {
    $("waiting-when").textContent = "";
  } else if (status.installs_from <= Date.now()) {
    $("waiting-when").textContent = "It installs on its own after tonight's backup, or you can install it now.";
  } else {
    $("waiting-when").textContent = `It installs on its own after the nightly backup on or after ${dateOf(status.installs_from)}, or you can install it now.`;
  }
}

function render() {
  $("sign-in").hidden = true;
  $("manage").hidden = false;
  $("installed").textContent = `This server runs Accordion ${status.installed}.`;

  const installing = status.installing;
  $("installing").hidden = !installing;
  if (installing) {
    $("installing-title").textContent = `Installing Accordion ${installing.version}`;
  }

  const newest = status.newest;
  $("waiting").hidden = !status.waiting || Boolean(installing);
  if (status.waiting) {
    $("summary").textContent = "";
    renderWaiting();
  } else if (newest) {
    $("summary").textContent = `Accordion is up to date. The newest release, ${newest.version}, came out ${day(newest.published)}.`;
  } else if (status.last_check && !status.last_check.problem) {
    $("summary").textContent = "Accordion is up to date. There's no newer release yet.";
  } else {
    $("summary").textContent = "";
  }
  if (status.install_problem) {
    showError(status.install_problem);
  }

  for (const radio of document.querySelectorAll("input[name=automatic]")) {
    radio.checked = (radio.value === "on") === status.automatic;
  }

  const check = status.last_check;
  if (!check) {
    $("last-check").textContent = "Accordion checks for updates once a day. It hasn't checked yet.";
  } else if (check.problem) {
    $("last-check").textContent = `The last check for updates, ${when(check.at)}, didn't work. ${check.problem}. Accordion tries again in a few hours.`;
  } else {
    $("last-check").textContent = `Accordion checks for updates once a day. It last checked ${when(check.at)}.`;
  }

  $("history-section").hidden = status.history.length === 0;
  $("history").replaceChildren(
    ...status.history.map((report) => {
      const item = document.createElement("li");
      item.textContent = describe(report);
      return item;
    }),
  );
}

async function load() {
  try {
    const { status: code, data } = await api("GET", "/api/updates");
    if (code === 401) {
      $("sign-in").hidden = false;
      $("manage").hidden = true;
      return;
    }
    if (code !== 200) throw new Error(data.error || PROBLEM);
    status = data;
    render();
  } catch (error) {
    // While an update installs, the server stops for a while. Keep waiting.
    if (!status?.installing) showError(error.message);
  }
  if (status?.installing) setTimeout(load, INSTALLING_MS);
}

$("sign-in").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  const button = event.submitter;
  button.disabled = true;
  try {
    const { status: code, data } = await api("POST", "/api/session", {
      username: $("username").value,
      password: $("password").value,
    });
    if (code !== 200) throw new Error(data.error || PROBLEM);
    $("password").value = "";
    await load();
  } catch (error) {
    showError(error.message);
  } finally {
    button.disabled = false;
  }
});

$("check").addEventListener("click", async () => {
  showError("");
  $("check").disabled = true;
  $("check-progress").hidden = false;
  try {
    const { status: code, data } = await api("POST", "/api/updates/check");
    if (code !== 200) throw new Error(data.error || PROBLEM);
    status = data;
    render();
  } catch (error) {
    showError(error.message);
  } finally {
    $("check").disabled = false;
    $("check-progress").hidden = true;
  }
});

$("install").addEventListener("click", async () => {
  showError("");
  $("install").disabled = true;
  $("install").textContent = "Downloading";
  try {
    const { status: code, data } = await api("POST", "/api/updates/install");
    if (code !== 202) throw new Error(data.error || PROBLEM);
    await load();
  } catch (error) {
    showError(error.message);
  } finally {
    $("install").disabled = false;
    $("install").textContent = "Install now";
  }
});

for (const radio of document.querySelectorAll("input[name=automatic]")) {
  radio.addEventListener("change", async () => {
    showError("");
    try {
      const { status: code, data } = await api("PUT", "/api/updates/settings", {
        automatic: radio.value === "on",
      });
      if (code !== 204) throw new Error(data.error || PROBLEM);
      await load();
    } catch (error) {
      showError(error.message);
    }
  });
}

load();
