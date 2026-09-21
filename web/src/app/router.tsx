import { createRouter, defineRoute } from "@solidjs/router";

import { AdminPage } from "../pages/AdminPage";
import { DocumentationPage } from "../pages/DocumentationPage";
import { HealthPage } from "../pages/HealthPage";
import { ProjectPage } from "../pages/ProjectPage";
import { ProjectsPage } from "../pages/ProjectsPage";
import { QueuePage } from "../pages/QueuePage";
import { SubjectPage } from "../pages/SubjectPage";

export const Router = createRouter({
  routes: [
    defineRoute({ path: "/", component: ProjectsPage }),
    defineRoute({ path: "/admin", component: AdminPage }),
    defineRoute({ path: "/queue", component: QueuePage }),
    defineRoute({ path: "/docs", component: DocumentationPage }),
    defineRoute({ path: "/health", component: HealthPage }),
    defineRoute({ path: "/projects/:projectId", component: ProjectPage }),
    defineRoute({ path: "/projects/:projectId/:kind/:key", component: SubjectPage }),
  ],
});
