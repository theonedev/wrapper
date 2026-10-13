package io.onedev.wrapper;

import java.lang.reflect.InvocationTargetException;
import java.util.Arrays;

/** Launch a Java main class through the Wrapper lifecycle bridge. */
public final class WrapperSimpleApp {
    private WrapperSimpleApp() {
    }

    public static void main(String[] args) {
        try {
            if (args.length == 0)
                throw new IllegalArgumentException("Set wrapper.app.parameter.1 to the application main class");
            WrapperManager.initialize();
            Class.forName(args[0]).getMethod("main", String[].class)
                    .invoke(null, (Object) Arrays.copyOfRange(args, 1, args.length));
        } catch (Throwable e) {
            if (e instanceof InvocationTargetException && e.getCause() != null)
                e = e.getCause();
            e.printStackTrace();
            WrapperManager.failAndReturn();
        }
    }
}
