import { Router, type IRouter } from "express";
import {
  computeGreeks,
  aggregatePortfolioGreeks,
  type OptionPosition,
} from "@tensor/core";

export const greeksRoutes: IRouter = Router();

/* ------------------------------------------------------------------ */
/*  POST /v1/greeks/compute                                           */
/*  Compute Greeks for individual options positions.                  */
/* ------------------------------------------------------------------ */
greeksRoutes.post("/compute", (req, res) => {
  try {
    const { positions } = req.body as { positions: OptionPosition[] };

    if (!positions || !Array.isArray(positions) || positions.length === 0) {
      res.status(400).json({ error: "positions array is required and must not be empty" });
      return;
    }

    for (const p of positions) {
      if (!p.asset || !p.option_type || !p.strike || !p.expiry || !p.underlying_price) {
        res.status(400).json({
          error: "Each position requires: asset, option_type, strike, expiry, underlying_price, implied_volatility",
        });
        return;
      }
    }

    const results = positions.map((p) => computeGreeks(p));
    res.json({ positions: results });
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    res.status(500).json({ error: message });
  }
});

/* ------------------------------------------------------------------ */
/*  POST /v1/greeks/portfolio                                         */
/*  Aggregate portfolio Greeks across all positions.                  */
/* ------------------------------------------------------------------ */
greeksRoutes.post("/portfolio", (req, res) => {
  try {
    const { positions } = req.body as { positions: OptionPosition[] };

    if (!positions || !Array.isArray(positions) || positions.length === 0) {
      res.status(400).json({ error: "positions array is required and must not be empty" });
      return;
    }

    res.json(aggregatePortfolioGreeks(positions));
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown error";
    res.status(500).json({ error: message });
  }
});
