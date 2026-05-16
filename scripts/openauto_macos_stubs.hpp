#pragma once
// BluetoothAdapterLister is referenced in SettingsWindow.cpp under Q_OS_MAC
// but never declared in the openauto headers.
// Guard with __has_include so the class is only defined when Qt include paths
// are on the compiler search path (i.e., during real builds, not cmake tests).
#if __has_include(<QStringList>)
#include <QStringList>
class BluetoothAdapterLister {
public:
    QStringList listAdapters() { return {}; }
};
#endif
