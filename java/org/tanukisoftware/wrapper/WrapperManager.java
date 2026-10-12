package org.tanukisoftware.wrapper;

/** Independent compatibility adapter for the JSW lifecycle API. */
public final class WrapperManager {
    private WrapperManager() {
    }

    /** Restart the JVM asynchronously, running shutdown hooks first. */
    public static void restartAndReturn() {
        org.servicewrapper.WrapperManager.restartAndReturn();
    }

    /** Stop the service asynchronously, running shutdown hooks first. */
    public static void stopAndReturn(int exitCode) {
        org.servicewrapper.WrapperManager.stopAndReturn(exitCode);
    }
}
