// Host for the native probe: ART loads the library, the library does the work.
//
// `System.load` is what triggers `JNI_OnLoad`, which is the only place a native
// library is handed the `JavaVM`.
public class Probe {
    public static native String run();

    public static void main(String[] args) {
        System.load(args.length > 0 ? args[0] : "/data/local/tmp/wb/libart_probe.so");
        String report = run();
        System.out.print(report);
        System.out.println(report.contains("FAILED") || report.contains("failed")
                ? "probe reported failures"
                : "ART loaded and bound every generated class");
    }
}
