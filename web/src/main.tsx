import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { createBrowserRouter, RouterProvider } from "react-router-dom";

import "./index.css";
import Start from "./routes/Start";
import Witness from "./routes/Witness";
import Deal from "./routes/Deal";
import Receipt from "./routes/Receipt";

const router = createBrowserRouter([
  { path: "/", element: <Start /> },
  { path: "/witness/:sessionId", element: <Witness /> },
  { path: "/deal/:dealId", element: <Deal /> },
  // The public receipt is deliberately reachable with no account and no token.
  { path: "/v/:receiptId", element: <Receipt /> },
]);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>,
);
