(() => {
if (window.__LANKUU_UI_BOOTED__) return;
window.__LANKUU_UI_BOOTED__ = true;

const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke;
const listen = tauri?.event?.listen;
const openDialog = tauri?.dialog?.open;
const isTauri = Boolean(invoke);
const nativeWindow = tauri?.window?.getCurrentWindow?.();

const state = {
  devices: [],
  selected: null,
  transfers: new Map(),
  messages: new Map(),
  receiverRunning: false,
  mirrorRunning: false,
  mirrorAddress: "",
  activeView: "share",
  unreadMessages: 0,
};

const elements = {
  deviceList: document.querySelector("#device-list"),
  refresh: document.querySelector("#refresh-devices"),
  manualHost: document.querySelector("#manual-host"),
  manualUse: document.querySelector("#use-manual-host"),
  selectedCard: document.querySelector("#selected-card"),
  selectedName: document.querySelector("#selected-name"),
  selectedAddress: document.querySelector("#selected-address"),
  dropZone: document.querySelector("#drop-zone"),
  dropDestination: document.querySelector("#drop-destination"),
  messageDestination: document.querySelector("#message-destination"),
  messageInput: document.querySelector("#message-input"),
  characterCount: document.querySelector("#character-count"),
  sendMessage: document.querySelector("#send-message"),
  receiverToggle: document.querySelector("#receiver-toggle"),
  receiverControl: document.querySelector(".receiver-control"),
  receiverLabel: document.querySelector("#receiver-label"),
  receiverCaption: document.querySelector("#receiver-caption"),
  transferList: document.querySelector("#transfer-list"),
  clearCompleted: document.querySelector("#clear-completed"),
  navShare: document.querySelector("#nav-share"),
  navMirror: document.querySelector("#nav-mirror"),
  navInbox: document.querySelector("#nav-inbox"),
  shareView: document.querySelector("#share-view"),
  mirrorView: document.querySelector("#mirror-view"),
  inboxView: document.querySelector("#inbox-view"),
  mirrorToggle: document.querySelector("#mirror-toggle"),
  mirrorToggleLabel: document.querySelector("#mirror-toggle span"),
  mirrorStatusLabel: document.querySelector("#mirror-status-label"),
  mirrorStatusCaption: document.querySelector("#mirror-status-caption"),
  mirrorAddress: document.querySelector("#mirror-address"),
  mirrorPort: document.querySelector("#mirror-port"),
  copyMirrorAddress: document.querySelector("#copy-mirror-address"),
  inboxBadge: document.querySelector("#inbox-badge"),
  inboxMessageCount: document.querySelector("#inbox-message-count"),
  inboxMessageLabel: document.querySelector("#inbox-message-label"),
  messageList: document.querySelector("#message-list"),
  clearInbox: document.querySelector("#clear-inbox"),
  footerStatus: document.querySelector("#footer-status"),
  toast: document.querySelector("#toast"),
};

let toastTimer;

function showToast(message) {
  elements.toast.textContent = message;
  elements.toast.classList.add("visible");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => elements.toast.classList.remove("visible"), 3200);
}

function reportActionError(action, error) {
  console.error(`${action} failed`, error);
  showToast(`${action} failed: ${error}`);
}

function initials(name) {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0])
    .join("") || "K";
}

function renderDevices() {
  elements.deviceList.replaceChildren();
  if (!state.devices.length) {
    const empty = document.createElement("div");
    empty.className = "device-empty";
    empty.innerHTML = '<span class="empty-device-icon"></span><strong>No devices found</strong><small>Start a receiver on another device, then refresh.</small>';
    elements.deviceList.append(empty);
    return;
  }

  state.devices.forEach((device) => {
    const row = document.createElement("button");
    row.className = `device-row${state.selected?.address === device.address ? " selected" : ""}`;
    row.type = "button";

    const avatar = document.createElement("span");
    avatar.className = "device-avatar";
    avatar.textContent = initials(device.name);

    const copy = document.createElement("span");
    copy.className = "device-copy";
    const name = document.createElement("strong");
    name.textContent = device.name;
    const address = document.createElement("small");
    address.textContent = `${device.address}:${device.port}`;
    copy.append(name, address);

    const dot = document.createElement("span");
    dot.className = "online-dot";
    row.append(avatar, copy, dot);
    row.addEventListener("click", () => selectDevice(device));
    elements.deviceList.append(row);
  });
}

function selectDevice(device) {
  state.selected = device;
  elements.selectedCard.classList.add("active");
  elements.selectedName.textContent = device.name;
  elements.selectedAddress.textContent = `${device.address}:${device.port}`;
  elements.dropDestination.textContent = `Send to ${device.name}`;
  elements.messageDestination.textContent = `To ${device.name}`;
  updateMessageButton();
  renderDevices();
}

function updateMessageButton() {
  elements.sendMessage.disabled = !state.selected || !elements.messageInput.value.trim();
}

async function discoverDevices() {
  elements.refresh.classList.add("loading");
  elements.footerStatus.textContent = "Looking for nearby receivers…";
  try {
    if (!isTauri) {
      await new Promise((resolve) => setTimeout(resolve, 450));
      state.devices = [
        { name: "Pixel 9", address: "192.168.1.42", port: 45454 },
        { name: "Ubuntu Studio", address: "192.168.1.56", port: 45454 },
      ];
    } else {
      state.devices = await invoke("discover_devices");
    }
    renderDevices();
    elements.footerStatus.textContent = state.devices.length
      ? `${state.devices.length} device${state.devices.length === 1 ? "" : "s"} nearby`
      : "No receivers found on this network";
    if (state.devices.length && !state.selected) selectDevice(state.devices[0]);
  } catch (error) {
    showToast(`Discovery failed: ${error}`);
    elements.footerStatus.textContent = "Discovery unavailable";
  } finally {
    elements.refresh.classList.remove("loading");
  }
}

function useManualAddress() {
  const address = elements.manualHost.value.trim();
  if (!address) return;
  selectDevice({ name: "Manual device", address, port: 45454 });
  showToast(`Using ${address}`);
}

async function chooseFiles() {
  if (!state.selected) {
    showToast("Choose a destination first.");
    return;
  }
  if (!isTauri) {
    addDemoTransfer("LanKuu-demo.zip", 28_600_000);
    return;
  }
  try {
    const result = await openDialog({ multiple: true, directory: false });
    if (!result) return;
    const paths = Array.isArray(result) ? result : [result];
    await sendPaths(paths);
  } catch (error) {
    showToast(`Could not choose files: ${error}`);
  }
}

async function sendPaths(paths) {
  if (!state.selected || !paths.length) {
    showToast("Choose a destination first.");
    return;
  }
  elements.footerStatus.textContent = "Sending files…";
  try {
    const count = await invoke("send_files", {
      host: state.selected.address,
      port: state.selected.port,
      paths,
    });
    showToast(`${count} file${count === 1 ? "" : "s"} delivered.`);
    elements.footerStatus.textContent = "Transfer complete";
  } catch (error) {
    showToast(`Transfer failed: ${error}`);
    elements.footerStatus.textContent = "Transfer failed";
  }
}

async function sendMessage() {
  const text = elements.messageInput.value.trim();
  if (!state.selected || !text) return;
  elements.sendMessage.disabled = true;
  elements.footerStatus.textContent = "Sending message…";
  try {
    if (isTauri) {
      await invoke("send_text", {
        host: state.selected.address,
        port: state.selected.port,
        text,
      });
    } else {
      await new Promise((resolve) => setTimeout(resolve, 450));
      addDemoTransfer("Text message", new TextEncoder().encode(text).length, "text");
    }
    elements.messageInput.value = "";
    elements.characterCount.textContent = "0";
    showToast("Message delivered.");
    elements.footerStatus.textContent = "Message delivered";
  } catch (error) {
    showToast(`Message failed: ${error}`);
    elements.footerStatus.textContent = "Message failed";
  } finally {
    updateMessageButton();
  }
}

async function toggleReceiver() {
  elements.receiverToggle.disabled = true;
  try {
    if (isTauri) {
      const status = await invoke(state.receiverRunning ? "stop_receiver" : "start_receiver");
      setReceiverStatus(status.running, status.directory);
    } else {
      setReceiverStatus(!state.receiverRunning, "~/Downloads/LanKuu");
    }
  } catch (error) {
    showToast(`${state.receiverRunning ? "Stop" : "Start"} failed: ${error}`);
  } finally {
    elements.receiverToggle.disabled = false;
  }
}

function setReceiverStatus(running, directory) {
  state.receiverRunning = running;
  elements.receiverToggle.setAttribute("aria-checked", String(running));
  elements.receiverControl.classList.toggle("active", running);
  elements.receiverLabel.textContent = running ? "Ready to receive" : "Not receiving";
  elements.receiverCaption.textContent = running
    ? "Visible to devices nearby"
    : "Turn on to appear nearby";
  elements.footerStatus.textContent = running
    ? `Receiving into ${directory || "Downloads/LanKuu"}`
    : "Ready on your local network";
}

function switchView(view) {
  state.activeView = view;
  elements.shareView.hidden = view !== "share";
  elements.mirrorView.hidden = view !== "mirror";
  elements.inboxView.hidden = view !== "inbox";
  elements.navShare.classList.toggle("active", view === "share");
  elements.navMirror.classList.toggle("active", view === "mirror");
  elements.navInbox.classList.toggle("active", view === "inbox");
  if (view === "inbox") {
    state.unreadMessages = 0;
    updateInboxBadge();
  }
}

function setMirrorStatus(status) {
  const running = Boolean(status?.running);
  state.mirrorRunning = running;
  state.mirrorAddress = status?.address || state.mirrorAddress || "Your desktop IP";
  elements.mirrorAddress.textContent = state.mirrorAddress;
  elements.mirrorPort.textContent = String(status?.port || 45456);
  elements.mirrorStatusLabel.textContent = running ? "Waiting for Android" : "Receiver is off";
  elements.mirrorStatusCaption.textContent = running
    ? "The player is ready. Start casting from your phone."
    : "Start the receiver before casting from your phone.";
  elements.mirrorToggle.classList.toggle("stop", running);
  elements.mirrorToggleLabel.textContent = running ? "Stop mirror receiver" : "Start mirror receiver";
}

async function toggleMirrorReceiver() {
  elements.mirrorToggle.disabled = true;
  try {
    if (isTauri) {
      const command = state.mirrorRunning ? "stop_mirror_receiver" : "start_mirror_receiver";
      setMirrorStatus(await invoke(command));
    } else {
      setMirrorStatus({ running: !state.mirrorRunning, address: "192.168.1.56", port: 45456 });
    }
    showToast(state.mirrorRunning ? "Mirror receiver started." : "Mirror receiver stopped.");
  } catch (error) {
    reportActionError(state.mirrorRunning ? "Stop mirror receiver" : "Start mirror receiver", error);
  } finally {
    elements.mirrorToggle.disabled = false;
  }
}

async function copyMirrorAddress() {
  if (!state.mirrorAddress || state.mirrorAddress === "Your desktop IP") {
    showToast("Could not determine this device's IP address.");
    return;
  }
  try {
    await navigator.clipboard.writeText(state.mirrorAddress);
    showToast("Desktop IP copied.");
  } catch (error) {
    reportActionError("Copy IP", error);
  }
}

function updateInboxBadge() {
  elements.inboxBadge.textContent = String(state.unreadMessages);
  elements.inboxBadge.hidden = state.unreadMessages === 0;
}

function addReceivedMessage(event) {
  if (state.messages.has(event.id)) return;
  state.messages.set(event.id, {
    id: event.id,
    sender: event.name.replace(/^Text from\s+/i, "") || "Nearby device",
    text: event.detail || "",
    receivedAt: new Date(),
  });
  if (state.activeView !== "inbox") state.unreadMessages += 1;
  updateInboxBadge();
  renderMessages();
}

function renderMessages() {
  elements.messageList.replaceChildren();
  const messages = [...state.messages.values()].sort((a, b) => b.receivedAt - a.receivedAt);
  elements.inboxMessageCount.textContent = String(messages.length);
  elements.inboxMessageLabel.textContent = messages.length === 1 ? "message" : "messages";
  if (!messages.length) {
    const empty = document.createElement("div");
    empty.className = "inbox-empty";
    empty.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 5h16v14H4V5Zm0 9h4l2 2h4l2-2h4" /></svg><strong>No messages received</strong><p>Turn on receiving, then send text from the LanKuu Android app.</p>';
    elements.messageList.append(empty);
    return;
  }

  messages.forEach((message) => {
    const row = document.createElement("article");
    row.className = "received-message";

    const avatar = document.createElement("span");
    avatar.className = "message-avatar";
    avatar.textContent = "Aa";

    const content = document.createElement("div");
    content.className = "message-content";
    const heading = document.createElement("div");
    heading.className = "message-heading";
    const sender = document.createElement("strong");
    sender.textContent = message.sender;
    const time = document.createElement("time");
    time.dateTime = message.receivedAt.toISOString();
    time.textContent = message.receivedAt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    heading.append(sender, time);
    const body = document.createElement("div");
    body.className = "message-body";
    body.textContent = message.text;
    content.append(heading, body);

    const copy = document.createElement("button");
    copy.className = "copy-message";
    copy.type = "button";
    copy.textContent = "Copy";
    copy.addEventListener("click", () => copyMessageText(message.text));

    row.append(avatar, content, copy);
    elements.messageList.append(row);
  });
}

async function copyMessageText(text) {
  try {
    await navigator.clipboard.writeText(text);
    showToast("Message copied.");
  } catch (error) {
    const field = document.createElement("textarea");
    field.value = text;
    field.style.position = "fixed";
    field.style.opacity = "0";
    document.body.append(field);
    field.select();
    const copied = document.execCommand("copy");
    field.remove();
    if (copied) showToast("Message copied.");
    else reportActionError("Copy message", error);
  }
}

function updateTransfer(event) {
  const previous = state.transfers.get(event.id) || {};
  state.transfers.set(event.id, {
    ...previous,
    ...event,
    updatedAt: Date.now(),
  });
  if (event.direction === "receive" && event.kind === "text" && event.status === "complete") {
    addReceivedMessage(event);
  }
  renderTransfers();
  if (event.status === "complete") {
    elements.footerStatus.textContent = `${event.name} complete`;
    if (event.direction === "receive" && event.kind === "text") {
      showToast(`New text from ${event.name.replace(/^Text from\s+/i, "")}. Open Inbox to read it.`);
    } else if (event.direction === "receive") {
      showToast(`${event.name} received.`);
    }
  } else if (event.status === "failed") {
    showToast(`${event.name} failed: ${event.detail || "Unknown error"}`);
  }
}

function renderTransfers() {
  elements.transferList.replaceChildren();
  const transfers = [...state.transfers.values()].sort((a, b) => b.updatedAt - a.updatedAt);
  if (!transfers.length) {
    const empty = document.createElement("div");
    empty.className = "activity-empty";
    empty.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 6v12m-4-4 4 4 4-4M5 4h14" /></svg><strong>No transfers yet</strong><small>Sent and received items will appear here.</small>';
    elements.transferList.append(empty);
    return;
  }

  transfers.forEach((transfer) => {
    const total = Number(transfer.total || 0);
    const moved = Number(transfer.transferred || 0);
    const percent = total ? Math.min(100, Math.round((moved / total) * 100)) : transfer.status === "complete" ? 100 : 0;
    const row = document.createElement("div");
    row.className = `transfer-item ${transfer.status}`;

    const icon = document.createElement("span");
    icon.className = `transfer-icon${transfer.status === "failed" ? " failed" : ""}`;
    icon.textContent = transfer.status === "failed" ? "!" : transfer.kind === "text" ? "T" : "↗";

    const main = document.createElement("div");
    main.className = "transfer-main";
    const top = document.createElement("div");
    top.className = "transfer-topline";
    const name = document.createElement("strong");
    name.textContent = transfer.name;
    const percentage = document.createElement("span");
    percentage.className = "transfer-percent";
    percentage.textContent = transfer.status === "failed" ? "Failed" : transfer.status === "complete" ? "Done" : `${percent}%`;
    top.append(name, percentage);

    const track = document.createElement("div");
    track.className = "progress-track";
    const fill = document.createElement("span");
    fill.style.width = `${percent}%`;
    track.append(fill);

    const meta = document.createElement("div");
    meta.className = "transfer-meta";
    const size = document.createElement("span");
    size.textContent = total ? `${formatBytes(moved)} of ${formatBytes(total)}` : transfer.kind === "text" ? "Text" : "Waiting";
    const detail = document.createElement("span");
    detail.className = "transfer-detail";
    detail.textContent = transfer.kind === "text" && transfer.direction === "receive"
      ? "Available in Inbox"
      : transfer.detail || (transfer.direction === "send" ? "Sent nearby" : "Received nearby");
    meta.append(size, detail);
    main.append(top, track, meta);

    const direction = document.createElement("span");
    direction.className = "direction-badge";
    direction.textContent = transfer.direction === "send" ? "↑" : "↓";
    row.append(icon, main, direction);
    elements.transferList.append(row);
  });
}

function formatBytes(value) {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const amount = value / 1024 ** index;
  return `${amount >= 10 || index === 0 ? amount.toFixed(0) : amount.toFixed(1)} ${units[index]}`;
}

function addDemoTransfer(name, total, kind = "file") {
  const id = Date.now();
  let moved = 0;
  updateTransfer({ id, direction: "send", kind, name, transferred: moved, total, status: "transferring" });
  const timer = setInterval(() => {
    moved = Math.min(total, moved + total * 0.12);
    updateTransfer({ id, direction: "send", kind, name, transferred: moved, total, status: moved >= total ? "complete" : "transferring" });
    if (moved >= total) clearInterval(timer);
  }, 150);
}

function addDemoIncomingMessage() {
  updateTransfer({
    id: "demo-incoming-text",
    direction: "receive",
    kind: "text",
    name: "Text from 192.168.1.42",
    transferred: 92,
    total: 92,
    status: "complete",
    detail: "Here is the address for tomorrow:\nhttps://maps.example.com/meeting-point",
  });
}

function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem("lankuu-theme", theme);
  } catch (error) {
    console.warn("Could not save appearance preference", error);
  }
}

function savedTheme() {
  try {
    return localStorage.getItem("lankuu-theme");
  } catch (error) {
    console.warn("Could not read appearance preference", error);
    return null;
  }
}

async function registerNativeEvents() {
  if (!isTauri) return;
  await listen("transfer-event", (event) => updateTransfer(event.payload));
  await listen("receiver-status", (event) => setReceiverStatus(event.payload.running, event.payload.directory));
  await listen("mirror-status", (event) => setMirrorStatus(event.payload));
  await listen("mirror-peer-status", (event) => {
    if (!state.mirrorRunning) return;
    elements.mirrorStatusLabel.textContent = event.payload;
    elements.mirrorStatusCaption.textContent = event.payload === "Android connected securely"
      ? "WebRTC is adapting video quality to your local network."
      : "Keep LanKuu open on both devices while the session connects.";
  });
  await listen("app-error", (event) => showToast(event.payload));

  const currentWebview = tauri?.webviewWindow?.getCurrentWebviewWindow?.();
  if (currentWebview?.onDragDropEvent) {
    await currentWebview.onDragDropEvent((event) => {
      const type = event.payload.type;
      elements.dropZone.classList.toggle("dragging", type === "over");
      if (type === "drop") sendPaths(event.payload.paths || []);
    });
  }
}

function bindActions() {
  elements.navShare.addEventListener("click", () => switchView("share"));
  elements.navMirror.addEventListener("click", () => switchView("mirror"));
  elements.navInbox.addEventListener("click", () => switchView("inbox"));
  elements.mirrorToggle.addEventListener("click", toggleMirrorReceiver);
  elements.copyMirrorAddress.addEventListener("click", copyMirrorAddress);
  elements.clearInbox.addEventListener("click", () => {
    state.messages.clear();
    state.unreadMessages = 0;
    updateInboxBadge();
    renderMessages();
    showToast("Inbox cleared.");
  });
  elements.refresh.addEventListener("click", discoverDevices);
  elements.manualUse.addEventListener("click", useManualAddress);
  elements.manualHost.addEventListener("keydown", (event) => {
    if (event.key === "Enter") useManualAddress();
  });
  elements.dropZone.addEventListener("click", chooseFiles);
  elements.messageInput.addEventListener("input", () => {
    elements.characterCount.textContent = elements.messageInput.value.length.toLocaleString();
    updateMessageButton();
  });
  elements.messageInput.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") sendMessage();
  });
  elements.sendMessage.addEventListener("click", sendMessage);
  elements.receiverToggle.addEventListener("click", toggleReceiver);
  elements.clearCompleted.addEventListener("click", () => {
    for (const [id, transfer] of state.transfers) {
      if (transfer.status !== "transferring") state.transfers.delete(id);
    }
    renderTransfers();
  });
  document.querySelector("#theme-toggle").addEventListener("click", () => {
    const nextTheme = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    setTheme(nextTheme);
    showToast(`${nextTheme === "dark" ? "Dark" : "Light"} appearance enabled.`);
  });
  document.querySelector("#open-downloads").addEventListener("click", async () => {
    if (isTauri) {
      showToast("Opening received files…");
      try {
        await invoke("open_downloads");
      } catch (error) {
        reportActionError("Open received files", error);
      }
    } else {
      showToast("Received files open here in the desktop app.");
    }
  });
  document.querySelector("#close-window").addEventListener("click", async (event) => {
    event.stopPropagation();
    try {
      if (nativeWindow?.close) await nativeWindow.close();
      else if (invoke) await invoke("close_window");
    } catch (error) {
      reportActionError("Close window", error);
    }
  });
  document.querySelector("#minimize-window").addEventListener("click", async (event) => {
    event.stopPropagation();
    try {
      if (nativeWindow?.minimize) await nativeWindow.minimize();
      else if (invoke) await invoke("minimize_window");
    } catch (error) {
      reportActionError("Minimize window", error);
    }
  });
  document.querySelector("#maximize-window").addEventListener("click", async (event) => {
    event.stopPropagation();
    try {
      if (nativeWindow?.toggleMaximize) await nativeWindow.toggleMaximize();
      else if (invoke) await invoke("toggle_maximize_window");
    } catch (error) {
      reportActionError("Maximize window", error);
    }
  });
}

async function initialize() {
  bindActions();
  const prefersDark = typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches;
  setTheme(savedTheme() || (prefersDark ? "dark" : "light"));
  try {
    await registerNativeEvents();
  } catch (error) {
    console.error("Could not register native events", error);
    showToast(`Native events unavailable: ${error}`);
  }
  if (isTauri) {
    try {
      const status = await invoke("receiver_status");
      setReceiverStatus(status.running, status.directory);
      setMirrorStatus(await invoke("mirror_status"));
    } catch (error) {
      showToast(`Could not read receiver status: ${error}`);
    }
  }
  try {
    await discoverDevices();
  } catch (error) {
    console.error("Initial discovery failed", error);
  }
  if (!isTauri && new URLSearchParams(window.location.search).get("demo") === "inbox") {
    addDemoIncomingMessage();
  }
}

window.addEventListener("error", (event) => {
  console.error("LanKuu UI error", event.error || event.message);
  showToast(`UI error: ${event.message}`);
});

initialize().catch((error) => {
  console.error("LanKuu initialization failed", error);
  showToast(`Startup error: ${error}`);
});
})();
