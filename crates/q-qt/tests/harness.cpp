#include <cassert>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <QtCore/QCoreApplication>
#include <QtCore/QString>
#include "q-qt/src/lib.cxxqt.h"
#include "q-qt/src/bar_series.cxxqt.h"

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
    assert(version.toStdString() == "2026.9.24");

    // --- BarSeries tests ---
    BarSeries series;
    assert(series.getBar_count() == 0);
    assert(series.getRevision() == 0);
    assert(series.geometry_revision() == 0);
    assert(series.vertex_len() == 0);
    assert(series.vertex_ptr() == nullptr);

    // Load generated history (100 bars)
    bool loaded = series.load_history_sample(100);
    assert(loaded);
    assert(series.getBar_count() == 100);
    assert(series.getRevision() == 1);
    assert(series.geometry_revision() == 0); // geometry not rebuilt yet
    assert(!series.getHas_forming());
    assert(series.getLow() > 0.0);
    assert(series.getHigh() > series.getLow());

    // Set viewport and surface
    series.set_viewport(0, 100, series.getLow(), series.getHigh());
    series.set_surface(800.0f, 600.0f);

    // Rebuild geometry
    series.rebuild_geometry();
    assert(series.geometry_revision() == 1);
    assert(series.getRevision() == 1); // revision does NOT advance on rebuild

    const BarVertex* vptr = series.vertex_ptr();
    std::size_t vlen = series.vertex_len();
    assert(vptr != nullptr);
    assert(vlen == 100 * 12); // 100 buckets * 12 vertices per bucket

    // Assert the first and last vertices
    const BarVertex& first_v = vptr[0];
    const BarVertex& last_v = vptr[vlen - 1];
    assert(first_v.x >= 0.0f && first_v.x <= 800.0f);
    assert(first_v.y >= 0.0f && first_v.y <= 600.0f);
    assert(last_v.x >= 0.0f && last_v.x <= 800.0f);
    assert(last_v.y >= 0.0f && last_v.y <= 600.0f);
    assert(first_v.forming == 0.0f);
    assert(last_v.forming == 0.0f);

    // Assert that revision advances only on mutation
    series.rebuild_geometry();
    assert(series.getRevision() == 1);
    assert(series.geometry_revision() == 1);

    // Mutation: ingest a forming bar
    int64_t forming_time = series.getLast_time() + 60;
    double forming_open = series.getLast_price();
    double forming_high = forming_open + 3.0;
    double forming_low = forming_open - 2.0;
    double forming_close = forming_open + 1.5;
    bool forming_ok = series.ingest_forming_bar(forming_time, forming_open, forming_high, forming_low, forming_close, 50.0);
    assert(forming_ok);
    assert(series.getRevision() == 2); // advanced on mutation!
    assert(series.getHas_forming());
    assert(series.geometry_revision() == 1); // stale until rebuild

    // Rebuild geometry including the forming bar (bar range [0, 101))
    series.set_viewport(0, 101, series.getLow(), series.getHigh());
    series.rebuild_geometry();
    assert(series.geometry_revision() == 2);
    const BarVertex* updated_ptr = series.vertex_ptr();
    std::size_t updated_len = series.vertex_len();
    assert(updated_len == 101 * 12);
    // Last bucket should be forming
    assert(updated_ptr[updated_len - 1].forming == 1.0f);

    // Mutation: clear forming bar
    series.clear_forming_bar();
    assert(!series.getHas_forming());
    assert(series.getRevision() == 3); // advanced on mutation!

    // --- Split geometry API ---
    BarSeries split_series;
    assert(split_series.completed_vertex_ptr() == nullptr);
    assert(split_series.completed_vertex_len() == 0);
    assert(split_series.completed_geometry_revision() == 0);
    assert(split_series.forming_vertex_ptr() == nullptr);
    assert(split_series.forming_vertex_len() == 0);
    assert(split_series.forming_geometry_revision() == 0);

    split_series.load_history_sample(100);
    split_series.set_viewport(0, 101, split_series.getLow(), split_series.getHigh());
    split_series.set_surface(800.0f, 600.0f);
    split_series.rebuild_split_geometry();

    assert(split_series.completed_geometry_revision() == 1);
    assert(split_series.forming_geometry_revision() == 0);
    assert(split_series.completed_vertex_len() == 100 * 12);
    assert(split_series.forming_vertex_len() == 0);
    assert(split_series.completed_vertex_ptr() != nullptr);
    assert(split_series.forming_vertex_ptr() == nullptr);

    const BarVertex* completed_ptr = split_series.completed_vertex_ptr();
    int64_t completed_rev = split_series.completed_geometry_revision();

    int64_t forming_time_split = split_series.getLast_time() + 60;
    double forming_open_split = split_series.getLast_price();
    split_series.ingest_forming_bar(
        forming_time_split,
        forming_open_split,
        forming_open_split + 2.0,
        forming_open_split - 1.0,
        forming_open_split + 1.0,
        25.0);
    split_series.rebuild_split_geometry();

    assert(split_series.completed_vertex_ptr() == completed_ptr);
    assert(split_series.completed_geometry_revision() == completed_rev);
    assert(split_series.completed_vertex_len() == 100 * 12);
    assert(split_series.forming_vertex_len() == 12);
    assert(split_series.forming_geometry_revision() == 1);
    assert(split_series.forming_vertex_ptr() != nullptr);
    assert(split_series.forming_vertex_ptr()[11].forming == 1.0f);

    split_series.clear_forming_bar();
    split_series.rebuild_split_geometry();
    assert(split_series.forming_vertex_len() == 0);
    assert(split_series.forming_vertex_ptr() == nullptr);
    assert(split_series.forming_geometry_revision() == 2);
    assert(split_series.completed_geometry_revision() == completed_rev);

    split_series.ingest_completed_bar(
        forming_time_split,
        forming_open_split,
        forming_open_split + 2.0,
        forming_open_split - 1.0,
        forming_open_split + 1.0,
        25.0);
    split_series.set_viewport(0, 102, split_series.getLow(), split_series.getHigh());
    split_series.rebuild_split_geometry();
    assert(split_series.completed_geometry_revision() > completed_rev);
    assert(split_series.completed_vertex_len() == 101 * 12);

    // Optional: Real lake dataset testing if Q_LAKE_PATH is provided
    const char* lake_env = std::getenv("Q_LAKE_PATH");
    if (lake_env != nullptr && std::strlen(lake_env) > 0) {
        std::cout << "Loading real lake dataset from: " << lake_env << std::endl;
        BarSeries lake_series;
        bool lake_ok = lake_series.load_history_parquet(QString::fromUtf8(lake_env));
        if (lake_ok) {
            std::cout << "Lake dataset loaded successfully!" << std::endl;
            std::cout << "  bar_count: " << lake_series.getBar_count() << std::endl;
            std::cout << "  last_price: " << lake_series.getLast_price() << std::endl;
            std::cout << "  first_time: " << lake_series.getFirst_time() << std::endl;
            std::cout << "  last_time: " << lake_series.getLast_time() << std::endl;
            std::cout << "  low: " << lake_series.getLow() << std::endl;
            std::cout << "  high: " << lake_series.getHigh() << std::endl;
        } else {
            std::cerr << "Warning: Failed to load lake dataset from " << lake_env << std::endl;
        }
    }

    std::cout << "Qt harness assertions passed successfully." << std::endl;
    return 0;
}
