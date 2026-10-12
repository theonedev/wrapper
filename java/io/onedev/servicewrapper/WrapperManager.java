package io.onedev.servicewrapper;

/** Optional lifecycle API for applications launched by ServiceWrapper. */
public final class WrapperManager {
    private WrapperManager() {
    }

    /** Restart the JVM asynchronously, running shutdown hooks first. */
    public static void restartAndReturn() {
        org.tanukisoftware.wrapper.WrapperManager.restartAndReturn();
    }

    /** Stop the service asynchronously, running shutdown hooks first. */
    public static void stopAndReturn(int exitCode) {
        org.tanukisoftware.wrapper.WrapperManager.stopAndReturn(exitCode);
    }
}
