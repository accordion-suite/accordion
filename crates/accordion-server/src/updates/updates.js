"use strict";

const $ = (id) => document.getElementById(id);

const PROBLEM = "Accordion ran into a problem. Try again in a moment.";

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

// A release date such as "2026-11-02", as "November 2, 2026". Read as a
// local date, so it doesn't shift a day in time zones behind UTC.
function day(date) {
  const [year, month, dayOfMonth] = date.split("-").map(Number);
  return new Date(year, month - 1, dayOfMonth).toLocaleDateString(undefined, {
    dateStyle: "long",
  });
}

function render(status) {
  $("sign-in").hidden = true;
  $("manage").hidden = false;
  $("installed").textContent = `This server runs Accordion ${status.installed}.`;

  const newest = status.newest;
  $("waiting").hidden = !status.waiting;
  if (status.waiting) {
    $("summary").textContent = "";
    $("waiting-title").textContent = `Accordion ${newest.version} is available`;
    $("waiting-published").textContent = `Released ${day(newest.published)}.`;
    $("waiting-security").hidden = !newest.security;
    $("waiting-notes").textContent = newest.notes;
    $("waiting-link").href = newest.link;
  } else if (newest) {
    $("summary").textContent = `Accordion is up to date. The newest release, ${newest.version}, came out ${day(newest.published)}.`;
  } else if (status.last_check && !status.last_check.problem) {
    $("summary").textContent = "Accordion is up to date. There's no newer release yet.";
  } else {
    $("summary").textContent = "";
  }

  const check = status.last_check;
  if (!check) {
    $("last-check").textContent = "Accordion checks for updates once a day. It hasn't checked yet.";
  } else if (check.problem) {
    $("last-check").textContent = `The last check for updates, ${when(check.at)}, didn't work. ${check.problem}. Accordion tries again in a few hours.`;
  } else {
    $("last-check").textContent = `Accordion checks for updates once a day. It last checked ${when(check.at)}.`;
  }
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
    render(data);
  } catch (error) {
    showError(error.message);
  }
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
    render(data);
  } catch (error) {
    showError(error.message);
  } finally {
    $("check").disabled = false;
    $("check-progress").hidden = true;
  }
});

load();
