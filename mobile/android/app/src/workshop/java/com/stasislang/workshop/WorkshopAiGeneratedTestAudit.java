package com.stasislang.workshop;

import java.util.Collection;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

final class WorkshopAiGeneratedTestAudit {
    private static final Pattern OBSERVABLE_CALL = Pattern.compile(
            "\\b(render|tick|update_[A-Za-z0-9_]*|init|reset_[A-Za-z0-9_]*|on_code_swap)\\s*\\(");
    private static final Pattern COMPARISON = Pattern.compile("==|!=|<=|>=|<|>");
    private static final Pattern NUMBER = Pattern.compile("(?<![A-Za-z0-9_])-?[0-9]+");

    static final class Result {
        final int passed;
        final int total;
        final boolean observableBehavior;
        final boolean boundaryCoverage;
        final String evidence;

        Result(int passed, int total, boolean observableBehavior, boolean boundaryCoverage,
                String evidence) {
            this.passed = passed;
            this.total = total;
            this.observableBehavior = observableBehavior;
            this.boundaryCoverage = boundaryCoverage;
            this.evidence = evidence;
        }
    }

    private WorkshopAiGeneratedTestAudit() {}

    static Result audit(Collection<String> testSources,
            WorkshopAiVerificationPolicy.Decision policy) {
        StringBuilder source = new StringBuilder();
        if (testSources != null) for (String item : testSources) {
            if (item != null) source.append(item).append('\n');
        }
        boolean hasTests = source.length() > 0;
        boolean observable = OBSERVABLE_CALL.matcher(source).find();
        int comparisons = count(COMPARISON.matcher(source));
        int numbers = count(NUMBER.matcher(source));
        boolean boundaries = comparisons >= 3 && numbers >= 3;
        int total = policy.requiresBoundaryCoverage ? 3 : 2;
        int passed = (hasTests ? 1 : 0) + (observable ? 1 : 0)
                + (policy.requiresBoundaryCoverage && boundaries ? 1 : 0);
        String evidence = "tests=" + hasTests + ", observable=" + observable
                + ", comparisons=" + comparisons + ", numeric_cases=" + numbers;
        return new Result(passed, total, observable, boundaries, evidence);
    }

    private static int count(Matcher matcher) {
        int count = 0;
        while (matcher.find()) count += 1;
        return count;
    }
}
