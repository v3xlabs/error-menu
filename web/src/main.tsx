import "./style.css";

import { render } from "@solidjs/web";

import { AppShell } from "./app/AppShell";
import { Router } from "./app/router";

const rootElement = document.querySelector("#root");

if (rootElement === null) {
  throw new Error("Missing #root element.");
}

render(() => <Router>{routeProperties => <AppShell>{routeProperties.children}</AppShell>}</Router>, rootElement);
