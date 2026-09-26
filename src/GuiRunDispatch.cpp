#include "GuiRunDispatch.h"

#include <QObject>
#include <QPointer>
#include <QThread>

#include <memory>
#include <stdexcept>

namespace cao::gui {
run::RunObservation queuedObservation(QObject* target, run::RunObserver observer) {
    Q_ASSERT(target && target->thread() == QThread::currentThread());
    // Workers can outlive the window. A separate owned context keeps queue admission safe,
    // and deleteLater honors its thread affinity when the worker releases the last owner.
    auto context =
        std::shared_ptr<QObject>(new QObject, [](QObject* object) { object->deleteLater(); });
    // A delivery can drain several events, and an earlier observer can destroy its target.
    // Recheck per event as well as at queue entry before borrowing presentation state.
    auto guardedObserver = [target = QPointer<QObject>{target},
                            observer = std::move(observer)](const run::RunEvent& event) {
        if (target && observer) observer(event);
    };
    return {std::move(guardedObserver),
            [context = std::move(context),
             target = QPointer<QObject>{target}](std::function<void()> delivery) {
                // Only the target thread may inspect this guard; checking it on the worker
                // before posting would race the window's destruction.
                auto guardedDelivery = [target, delivery = std::move(delivery)] {
                    if (target) delivery();
                };
                if (!QMetaObject::invokeMethod(context.get(), std::move(guardedDelivery),
                                               Qt::QueuedConnection)) {
                    throw std::runtime_error("Could not queue GUI run observation");
                }
            }};
}
}  // namespace cao::gui
