import { Router, type IRouter } from "express";
import {
  calculateMargin,
  calculateHealth,
  deltaNet,
  type Position,
} from "@tensor/core";

export const marginRoutes: IRouter = Router();

/* ------------------------------------------------------------------ */
/*  POST /v1/margin/calculate                                         */
/*  Calculate margin requirements for a set of positions.             */
/* ------------------------------------------------------------------ */
marginRoutes.post("/calculate", (req, res) => {
  try {
    const { positions, collateral } = req.body as {
      positions: Position[];
      collateral: number;
    };

    if (!positions || !Array.isArray(positions) || positions.length === 0) {
      res.status(400).json({ error: "positions array is required and must not be empty" });
      return;
    }

    if (collateral === undefined || typeof collateral !== "number") {
      res.status(400).json({ error: "collateral (number) is required" });
      return;
    }

    res.json(calculateMargin(positions, collateral));
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    res.status(500).json({ error: message });
  }
});

/* ------------------------------------------------------------------ */
/*  POST /v1/margin/health                                            */
/*  Check margin health: equity, maintenance margin, liq distance.    */
/* ------------------------------------------------------------------ */
marginRoutes.post("/health", (req, res) => {
  try {
    const { positions, collateral, unrealized_pnl } = req.body as {
      positions: Position[];
      collateral: number;
      unrealized_pnl?: number;
    };

    if (!positions || !Array.isArray(positions)) {
      res.status(400).json({ error: "positions array is required" });
      return;
    }

    if (collateral === undefined || typeof collateral !== "number") {
      res.status(400).json({ error: "collateral (number) is required" });
      return;
    }

    res.json(calculateHealth(positions, collateral, unrealized_pnl));
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    res.status(500).json({ error: message });
  }
});

/* ------------------------------------------------------------------ */
/*  POST /v1/margin/delta-net                                         */
/*  Calculate delta-netted margin for hedged positions.               */
/* ------------------------------------------------------------------ */
marginRoutes.post("/delta-net", (req, res) => {
  try {
    const { positions } = req.body as { positions: Position[] };

    if (!positions || !Array.isArray(positions) || positions.length === 0) {
      res.status(400).json({ error: "positions array is required and must not be empty" });
      return;
    }

    res.json(deltaNet(positions));
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    res.status(500).json({ error: message });
  }
});
