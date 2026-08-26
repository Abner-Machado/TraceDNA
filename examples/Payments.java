/** A payment batch with a rare bug. Deterministic: the same SEED always behaves the same. */
public class Payments {

    static int bracket(int seed, int index) {
        return (seed * 7919 + index * 104729) % 100;
    }

    static int rate(int bracket, String currency) {
        // The BRL table was never filled in above bracket 96.
        if (currency.equals("BRL") && bracket >= 97) {
            return -1;
        }
        return 5 + bracket / 10;
    }

    static int fee(int amount, int rate) {
        return amount * rate / 100;
    }

    static String format(int index, int amount, int fee) {
        return String.format("payment %d: amount=%d fee=%d", index, amount, fee);
    }

    static void log(String message) {
        System.out.println(message);
    }

    public static void main(String[] args) {
        int seed = Integer.parseInt(System.getenv("SEED"));
        int amount = Integer.parseInt(System.getenv("INPUT").split(":")[1]);
        String currency = System.getenv("CURRENCY");
        int failures = 0;

        for (int index = 0; index < 8; index++) {
            int bracket = bracket(seed, index);
            int rate = rate(bracket, currency);
            if (rate < 0) {
                System.err.println("no " + currency + " rate for bracket " + bracket);
                failures++;
                continue;
            }
            log(format(index, amount, fee(amount, rate)));
        }
        System.exit(failures > 0 ? 1 : 0);
    }
}
