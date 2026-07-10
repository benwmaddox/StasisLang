package com.stasislang.workshop;

import java.math.BigDecimal;
import java.math.RoundingMode;

final class WorkshopMoney {
    private WorkshopMoney() {}

    static String formatUsd(double value) {
        if (!Double.isFinite(value)) throw new IllegalArgumentException("USD value must be finite");
        BigDecimal cents = BigDecimal.valueOf(Math.max(0.0, value))
                .setScale(2, RoundingMode.HALF_UP);
        return "$" + cents.toPlainString();
    }
}
