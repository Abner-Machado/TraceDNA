/** An invoice with a bug that never crashes. Deterministic: the same SEED always behaves the same. */
public class Invoice {

    static int price(int seed, int line) {
        return 20 + (seed * 31 + line * 17) % 80;
    }

    static int discount(int price, int seed) {
        // The loyalty discount was meant to be 12 percent and got typed in as 120 flat.
        if (seed % 13 == 0) {
            return 120;
        }
        return price / 10;
    }

    static int total(int price, int discount) {
        return price - discount;
    }

    static String describe(int line, int total) {
        return "line " + line + ": total=" + total;
    }

    public static void main(String[] args) {
        int seed = Integer.parseInt(System.getenv("SEED"));
        for (int line = 0; line < 4; line++) {
            int price = price(seed, line);
            System.out.println(describe(line, total(price, discount(price, seed))));
        }
    }
}
