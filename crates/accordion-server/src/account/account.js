"use strict";

const $ = (id) => document.getElementById(id);

const PROBLEM = "Accordion ran into a problem. Try again in a moment.";

const USES = {
  dav: "Files",
  mail: "Mail",
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

// Such as "October 21, 2026". The month is a word, so the date reads the
// same way in every country.
function day(iso) {
  return new Date(iso).toLocaleDateString(undefined, {
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

function renderPasswords(passwords) {
  const list = $("passwords");
  list.replaceChildren();
  $("none").hidden = passwords.length > 0;
  for (const password of passwords) {
    const item = document.createElement("li");
    const name = document.createElement("strong");
    name.textContent = password.device_name;
    const details = document.createElement("p");
    details.className = "hint";
    const used = password.last_used_at
      ? `last used ${day(password.last_used_at)}`
      : "not used yet";
    details.textContent = `${USES[password.use_for]}. Created ${day(password.created_at)}, ${used}.`;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "secondary";
    remove.textContent = "Delete";
    remove.addEventListener("click", () => deletePassword(password, remove));
    item.append(name, details, remove);
    list.append(item);
  }
}

async function load() {
  try {
    const session = await api("GET", "/api/session");
    if (session.status === 401) {
      $("account").hidden = true;
      $("sign-in").hidden = false;
      return;
    }
    if (session.status !== 200) {
      throw new Error(session.data.error || PROBLEM);
    }
    const { status, data } = await api("GET", "/api/app-passwords");
    if (status !== 200) {
      throw new Error(data.error || PROBLEM);
    }
    showError("");
    $("signed-in-as").textContent = session.data.user.username;
    $("change-username").value = session.data.user.username;
    $("dav-username").textContent = session.data.user.username;
    $("dav-address").textContent = `${location.origin}/dav/drive/`;
    renderPasswords(data);
    $("sign-in").hidden = true;
    $("account").hidden = false;
  } catch (error) {
    showError(error.message);
  }
}

// Creates the app password. Returns false if the user needs to confirm
// their password first.
async function create() {
  const { status, data } = await api("POST", "/api/app-passwords", {
    device_name: $("device-name").value,
    use_for: "dav",
  });
  if (status === 403) {
    return false;
  }
  if (status !== 201) {
    throw new Error(data.error || PROBLEM);
  }
  $("created-name").textContent = data.app_password.device_name;
  $("created-password").textContent = data.password;
  $("create").hidden = true;
  $("confirm").hidden = true;
  $("created").hidden = false;
  $("device-name").value = "";
  await load();
  return true;
}

async function withButton(event, action) {
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
}

async function deletePassword(password, button) {
  const sure = confirm(
    `Delete the app password for ${password.device_name}? That device won't be able to open your files until you give it a new one.`,
  );
  if (!sure) return;
  showError("");
  button.disabled = true;
  try {
    const { status, data } = await api("DELETE", `/api/app-passwords/${password.id}`);
    if (status !== 204) {
      throw new Error(data.error || PROBLEM);
    }
    await load();
  } catch (error) {
    showError(error.message);
    button.disabled = false;
  }
}

$("sign-in").addEventListener("submit", (event) =>
  withButton(event, async () => {
    const { status, data } = await api("POST", "/api/session", {
      username: $("username").value,
      password: $("password").value,
    });
    if (status !== 200) {
      throw new Error(data.error || PROBLEM);
    }
    $("password").value = "";
    await load();
  }),
);

$("create").addEventListener("submit", (event) =>
  withButton(event, async () => {
    if (!(await create())) {
      $("create").hidden = true;
      $("confirm").hidden = false;
      $("confirm-password").focus();
    }
  }),
);

$("confirm").addEventListener("submit", (event) =>
  withButton(event, async () => {
    const { status, data } = await api("POST", "/api/session/confirm", {
      password: $("confirm-password").value,
    });
    if (status !== 204) {
      throw new Error(data.error || PROBLEM);
    }
    $("confirm-password").value = "";
    if (!(await create())) {
      throw new Error(PROBLEM);
    }
  }),
);

$("change-password").addEventListener("submit", (event) =>
  withButton(event, async () => {
    $("changed").hidden = true;
    if ($("new-password").value !== $("repeat-password").value) {
      throw new Error("The two new passwords don't match. Type them again.");
    }
    const { status, data } = await api("POST", "/api/account/password", {
      current_password: $("current-password").value,
      new_password: $("new-password").value,
    });
    if (status !== 204) {
      throw new Error(data.error || PROBLEM);
    }
    for (const id of ["current-password", "new-password", "repeat-password"]) {
      $(id).value = "";
    }
    $("changed").hidden = false;
  }),
);

$("done").addEventListener("click", () => {
  $("created-password").textContent = "";
  $("created").hidden = true;
  $("create").hidden = false;
});

load();
