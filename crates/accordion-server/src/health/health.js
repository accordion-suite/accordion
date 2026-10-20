"use strict";

const $ = (id) => document.getElementById(id);

const REFRESH_MS = 60 * 1000;

const LABELS = {
  ok: "OK",
  warning: "Needs attention",
  problem: "Problem",
};

const OVERALL = {
  ok: "Everything's working.",
  warning: "Something needs your attention.",
  problem: "Something isn't working.",
};

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

function render(report) {
  $("overall").textContent = OVERALL[report.state];
  $("overall").dataset.state = report.state;
  const list = $("checks");
  list.replaceChildren();
  for (const check of report.checks) {
    const item = document.createElement("li");
    item.className = "check";
    item.dataset.state = check.state;
    const title = document.createElement("h2");
    title.textContent = check.title;
    const state = document.createElement("span");
    state.className = "state";
    state.textContent = LABELS[check.state];
    title.append(" ", state);
    const summary = document.createElement("p");
    summary.textContent = check.summary;
    item.append(title, summary);
    list.append(item);
  }
}

async function load() {
  try {
    const { status, data } = await api("GET", "/api/health");
    if (status === 401) {
      $("report").hidden = true;
      $("sign-in").hidden = false;
      return;
    }
    if (status !== 200) {
      throw new Error(data.error || "Accordion ran into a problem. Try again in a moment.");
    }
    showError("");
    $("sign-in").hidden = true;
    render(data);
    $("report").hidden = false;
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
    const { status, data } = await api("POST", "/api/session", {
      username: $("username").value,
      password: $("password").value,
    });
    if (status !== 200) {
      throw new Error(data.error || "Accordion ran into a problem. Try again in a moment.");
    }
    $("password").value = "";
    await load();
  } catch (error) {
    showError(error.message);
  } finally {
    button.disabled = false;
  }
});

$("refresh").addEventListener("click", load);

load();
setInterval(() => {
  if ($("sign-in").hidden) load();
}, REFRESH_MS);
