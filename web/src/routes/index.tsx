import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/')({
  beforeLoad: () => {
    throw redirect({ to: '/library', search: { sort: 'updated', dir: 'desc' } })
  },
})
