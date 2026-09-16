#include <cassert>
#include <iostream>
#include <QtCore/QCoreApplication>
#include <QtCore/QString>
#include "q-qt/src/lib.cxxqt.h"

int main(int argc, char** argv) {
    int fake_argc = 0;
    char* fake_argv[] = {nullptr};
    QCoreApplication app(fake_argc, fake_argv);

    CoreInfo info;
    QString version = info.getVersion();
    QString contracts_rev = info.getContracts_rev();

    std::cout << "CoreInfo version: " << version.toStdString() << std::endl;
    std::cout << "CoreInfo contracts_rev: " << contracts_rev.toStdString() << std::endl;

    assert(!version.isEmpty());
    assert(!contracts_rev.isEmpty());
    assert(version.toStdString() == "2026.9.15");

    std::cout << "Qt harness assertions passed successfully." << std::endl;
    return 0;
}
