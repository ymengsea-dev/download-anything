import React from "react";
import ReactDOM from "react-dom/client";
// Base styles first, so each screen's own stylesheet comes after them in dist/main.css.
import "./styles/base.css";
import App from "./app/App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
