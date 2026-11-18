// What the viewer (remote-view.js) takes from the page around it; in the
// main window app.js has these.
const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
