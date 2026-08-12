import { createBrowserRouter } from 'react-router'
import { RouterProvider } from 'react-router/dom'
import AdminLayout from './layouts/AdminLayout'
import HomeLayout from './layouts/HomeLayout'

const router = createBrowserRouter([
  {
    path: '/',
    element: <HomeLayout />,
    children: [{ index: true, element: <HomeIndex /> }],
  },
  {
    path: '/admin',
    element: <AdminLayout />,
    children: [{ index: true, element: <AdminIndex /> }],
  },
])

export default function App() {
  return <RouterProvider router={router} />
}

function HomeIndex() {
  return (
    <section className="page">
      <h1>Home</h1>
      <p>The PlatPulse Home shell renders and navigates on every supported viewport.</p>
    </section>
  )
}

function AdminIndex() {
  return (
    <section className="page">
      <h1>Admin</h1>
      <p>The PlatPulse Admin shell renders and navigates on every supported viewport.</p>
    </section>
  )
}
