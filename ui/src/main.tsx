import "./zod-csp";
import { CSPProvider } from "@base-ui/react/csp-provider";
import { createRoot } from "react-dom/client";
import { App } from "./workbench/App";
import "./workbench/workbench.css";

const root = document.getElementById("app");
if (root) createRoot(root).render(<CSPProvider disableStyleElements><App /></CSPProvider>);
