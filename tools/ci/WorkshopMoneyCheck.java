package com.stasislang.workshop;

public final class WorkshopMoneyCheck {
    public static void main(String[] args) {
        expect(0.004, "$0.00");
        expect(0.005, "$0.01");
        expect(0.014, "$0.01");
        expect(0.015, "$0.02");
        expect(-0.0, "$0.00");
        expect(5.0, "$5.00");
        System.out.println("android money display ok");
    }

    private static void expect(double value, String expected) {
        String actual = WorkshopMoney.formatUsd(value);
        if (!expected.equals(actual)) {
            throw new AssertionError(value + " formatted as " + actual + ", expected " + expected);
        }
    }
}
