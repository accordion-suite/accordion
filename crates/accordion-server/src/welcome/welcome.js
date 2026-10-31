"use strict";

const $ = (id) => document.getElementById(id);

const PROBLEM = "Accordion ran into a problem. Try again in a moment.";

// The link's secret, from the part after "#", which never reaches the
// server's logs.
const token = decodeURIComponent(location.hash.slice(1));

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
    throw new Error("Accordion didn't respond. Check that you're connected, then try again.");
  }
  const data = await response.json().catch(() => ({}));
  return { status: response.status, data };
}

async function load() {
  // Until the form shows, the error sits under the title.
  $("choose").before($("error"));
  if (!token) {
    showError("This link is missing its last part. Open the whole link from the message you were sent.");
    return;
  }
  try {
    const { status, data } = await api("POST", "/api/welcome/check", { token });
    if (status !== 200) throw new Error(data.error || PROBLEM);
    if (data.purpose === "reset") {
      $("title").textContent = "Choose a new password";
      $("greeting").textContent = `Hi ${data.display_name}. Your password was reset, so choose a new one.`;
    } else {
      $("greeting").textContent = `Hi ${data.display_name}. Choose a password to finish setting up your account.`;
    }
    $("welcome-username").textContent = data.username;
    $("username").value = data.username;
    $("choose").hidden = false;
    $("choose").querySelector(".actions").before($("error"));
    $("new-password").focus();
  } catch (error) {
    showError(error.message);
  }
}

$("choose").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  if ($("new-password").value !== $("repeat-password").value) {
    showError("The two passwords don't match. Type them again.");
    return;
  }
  const button = event.submitter;
  button.disabled = true;
  try {
    const { status, data } = await api("POST", "/api/welcome", {
      token,
      password: $("new-password").value,
    });
    if (status !== 200) throw new Error(data.error || PROBLEM);
    $("new-password").value = "";
    $("repeat-password").value = "";
    // The link is used up, so keep it out of the browser's history.
    history.replaceState(null, "", location.pathname);
    $("choose").hidden = true;
    $("title").textContent = "You're all set";
    $("done").hidden = false;
    $("title").focus();
  } catch (error) {
    showError(error.message);
    button.disabled = false;
  }
});

load();
