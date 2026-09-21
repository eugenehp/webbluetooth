// Load the generated callback classes in real ART and prove they work.
//
// `dexdump` parses a DEX; it does not link one. This runs on a device, so each
// class is resolved against the real `android.bluetooth` superclass, its
// methods are checked to be the native ones the crate will bind, and — the part
// most likely to be wrong — the hand-written constructor is actually invoked.
// The constructor is the only method in these classes with a `code_item`, four
// units written by hand, and nothing before this has ever executed it.

import java.lang.reflect.Constructor;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;

public class Driver {
    public static void main(String[] args) {
        int failed = 0;
        for (String name : args) {
            try {
                Class<?> c = Class.forName(name);
                System.out.println("  class     " + name);
                System.out.println("    super   " + c.getSuperclass().getName());

                Method[] methods = c.getDeclaredMethods();
                int natives = 0;
                for (Method m : methods) {
                    boolean isNative = Modifier.isNative(m.getModifiers());
                    if (isNative) {
                        natives++;
                    }
                    System.out.println("    method  " + (isNative ? "native " : "       ")
                            + m.getName() + "/" + m.getParameterCount());
                }
                if (natives != methods.length) {
                    System.out.println("    !! not every declared method is native");
                    failed++;
                }

                Constructor<?> ctor = c.getDeclaredConstructor();
                Object instance = ctor.newInstance();
                if (instance == null) {
                    System.out.println("    !! constructor returned null");
                    failed++;
                } else {
                    System.out.println("    ctor    ran, instance of "
                            + instance.getClass().getName());
                }
            } catch (Throwable t) {
                System.out.println("  FAIL      " + name + ": " + t);
                failed++;
            }
        }
        System.out.println(failed == 0
                ? "ART accepted every generated class"
                : failed + " failure(s)");
        System.exit(failed == 0 ? 0 : 1);
    }
}
