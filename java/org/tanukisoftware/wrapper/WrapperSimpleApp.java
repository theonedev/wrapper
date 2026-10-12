package org.tanukisoftware.wrapper;

/** Independent compatibility adapter for the JSW main class. */
public final class WrapperSimpleApp {
    private WrapperSimpleApp() {
    }

    public static void main(String[] args) {
        org.servicewrapper.WrapperSimpleApp.main(args);
    }
}
