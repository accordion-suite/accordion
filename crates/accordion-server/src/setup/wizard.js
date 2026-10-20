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

function show(id) {
  for (const section of ["claim", "name", "working", "done"]) {
    $(section).hidden = section !== id;
  }
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
      show("name");
  }
}

async function refresh() {
  try {
    const status = await api("GET", "/api/setup");
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
    if (error.message.startsWith("This setup page has expired")) {
      saveToken("");
    }
    showError(error.message);
  } finally {
    button.disabled = false;
  }
});

refresh();
