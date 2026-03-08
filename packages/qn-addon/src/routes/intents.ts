import { Router, type IRouter } from "express";
import {
  validateIntent,
  solveIntent,
  type TradingIntent,
  type SolverConstraints,
} from "@tensor/core";

export const intentRoutes: IRouter = Router();

/* ------------------------------------------------------------------ */
/*  POST /v1/intents/validate                                         */
/*  Validate a multi-leg trading intent.                              */
/* ------------------------------------------------------------------ */
intentRoutes.post("/validate", (req, res) => {
  try {
    const { intent } = req.body as { intent: TradingIntent };

    if (!intent) {
      res.status(400).json({ error: "intent object is required" });
      return;
    }

    if (!intent.legs || !Array.isArray(intent.legs) || intent.legs.length === 0) {
      res.status(400).json({ error: "intent.legs array is required and must not be empty" });
      return;
    }

    res.json(validateIntent(intent));
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    res.status(500).json({ error: message });
  }
});

/* ------------------------------------------------------------------ */
/*  POST /v1/intents/solve                                            */
/*  Decompose intent into optimal execution sequence.                 */
/* ------------------------------------------------------------------ */
intentRoutes.post("/solve", (req, res) => {
  try {
    const { intent, constraints } = req.body as {
      intent: TradingIntent;
      constraints?: SolverConstraints;
    };

    if (!intent) {
      res.status(400).json({ error: "intent object is required" });
      return;
    }

    if (!intent.legs || !Array.isArray(intent.legs) || intent.legs.length === 0) {
      res.status(400).json({ error: "intent.legs array is required and must not be empty" });
      return;
    }

    res.json(solveIntent(intent, constraints));
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    res.status(500).json({ error: message });
  }
});
