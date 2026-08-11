import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { initTheme } from "./lib/theme.svelte";

// Apply the saved theme before mount so the first paint uses the right palette.
initTheme();

export default mount(App, { target: document.getElementById("app")! });
