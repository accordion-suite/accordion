"use strict";

const $ = (id) => document.getElementById(id);

// The wizard passes its handoff after "#", so it never reaches the server's
// log. Remove it from the address bar once it's read.
const handoff = window.location.hash.slice(1) || null;
if (handoff) history.replaceState(null, "", window.location.pathname);

function showError(message) {
  $("error").textContent = message;
  $("error").hidden = !message;
}

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
  if (!response.ok) {
    throw new Error(data.error || "Accordion ran into a problem. Try again in a moment.");
  }
  return data;
}

async function start() {
  try {
    const status = await api("GET", "/api/setup");
    if (!status.owner_needed) {
      $("set-up").hidden = false;
      return;
    }
    $("code-field").hidden = Boolean(handoff) || !status.code_needed;
    $("owner").hidden = false;
  } catch (error) {
    showError(error.message);
  }
}

$("owner").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  if ($("password").value !== $("confirm").value) {
    showError("The passwords don't match. Type them again.");
    return;
  }
  const button = event.submitter;
  button.disabled = true;
  try {
    await api("POST", "/api/setup/owner", {
      handoff,
      code: $("code").value || null,
      display_name: $("display-name").value,
      username: $("username").value,
      password: $("password").value,
    });
    $("owner").hidden = true;
    $("finished").hidden = false;
  } catch (error) {
    // A handoff lasts five minutes. After that, the setup code still works.
    if (error.message.startsWith("That setup code isn't right") && handoff) {
      $("code-field").hidden = false;
      showError("This page has expired. Enter the setup code to continue.");
    } else {
      showError(error.message);
    }
  } finally {
    button.disabled = false;
  }
});

start();
