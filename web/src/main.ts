import "./fonts";
import "./tokens.css";
import "./base.css";

import { mount } from "svelte";

import App from "./App.svelte";
import { startRouter } from "./lib/router.svelte";
import { boot } from "./lib/session.svelte";

startRouter();
void boot();
mount(App, { target: document.body });
