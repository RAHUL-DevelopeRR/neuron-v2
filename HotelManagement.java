import javax.swing.*;
import javax.swing.border.*;
import javax.swing.table.*;
import java.awt.*;
import java.awt.event.*;
import java.awt.geom.*;
import java.util.*;

public class HotelManagement extends JFrame {
    private CardLayout cardLayout;
    private JPanel mainPanel;
    private JTable roomsTable;
    private DefaultTableModel tableModel;
    private java.util.List<Room> rooms = new ArrayList<>();
    private Color primaryColor = new Color(102, 126, 234);
    private Color secondaryColor = new Color(118, 75, 162);
    private Color accentColor = new Color(255, 107, 107);
    private Color successColor = new Color(78, 205, 196);
    private Color bgColor = new Color(245, 247, 250);
    
    public HotelManagement() {
        initializeRooms();
        setupUI();
    }
    
    private void initializeRooms() {
        for (int i = 101; i <= 110; i++) {
            String type = (i % 3 == 0) ? "Suite" : (i % 2 == 0) ? "Double" : "Single";
            double price = type.equals("Suite") ? 250.0 : type.equals("Double") ? 150.0 : 100.0;
            rooms.add(new Room(i, type, price, i % 4 == 0 ? "Occupied" : "Available"));
        }
    }
    
    private void setupUI() {
        setTitle("LuxeStay Hotel Management System");
        setSize(1200, 800);
        setDefaultCloseOperation(JFrame.EXIT_ON_CLOSE);
        setLocationRelativeTo(null);
        
        GradientPanel bgPanel = new GradientPanel();
        bgPanel.setLayout(new BorderLayout(10, 10));
        bgPanel.setBorder(new EmptyBorder(20, 20, 20, 20));
        
        JPanel sidebar = createSidebar();
        bgPanel.add(sidebar, BorderLayout.WEST);
        
        cardLayout = new CardLayout();
        mainPanel = new JPanel(cardLayout);
        mainPanel.setOpaque(false);
        
        mainPanel.add(createDashboardPanel(), "dashboard");
        mainPanel.add(createRoomsPanel(), "rooms");
        mainPanel.add(createBookingsPanel(), "bookings");
        mainPanel.add(createGuestsPanel(), "guests");
        
        bgPanel.add(mainPanel, BorderLayout.CENTER);
        add(bgPanel);
    }
    
    private JPanel createSidebar() {
        JPanel sidebar = new JPanel();
        sidebar.setLayout(new BoxLayout(sidebar, BoxLayout.Y_AXIS));
        sidebar.setBackground(new Color(0, 0, 0, 100));
        sidebar.setBorder(new EmptyBorder(20, 15, 20, 15));
        sidebar.setPreferredSize(new Dimension(220, 0));
        
        JLabel title = new JLabel("LuxeStay");
        title.setFont(new Font("Segoe UI", Font.BOLD, 24));
        title.setForeground(Color.WHITE);
        title.setAlignmentX(Component.CENTER_ALIGNMENT);
        sidebar.add(title);
        sidebar.add(Box.createVerticalStrut(30));
        
        String[] menuItems = {"Dashboard", "Rooms", "Bookings", "Guests"};
        String[] cards = {"dashboard", "rooms", "bookings", "guests"};
        
        for (int i = 0; i < menuItems.length; i++) {
            JButton btn = createSidebarButton(menuItems[i], cards[i]);
            sidebar.add(btn);
            sidebar.add(Box.createVerticalStrut(10));
        }
        
        sidebar.add(Box.createVerticalGlue());
        
        JLabel status = new JLabel("System Online");
        status.setFont(new Font("Segoe UI", Font.PLAIN, 12));
        status.setForeground(successColor);
        status.setAlignmentX(Component.CENTER_ALIGNMENT);
        sidebar.add(status);
        
        return sidebar;
    }
    
    private JButton createSidebarButton(String text, String card) {
        JButton btn = new JButton(text);
        btn.setFont(new Font("Segoe UI", Font.BOLD, 16));
        btn.setForeground(Color.WHITE);
        btn.setBackground(new Color(255, 255, 255, 30));
        btn.setBorder(BorderFactory.createCompoundBorder(
            BorderFactory.createLineBorder(new Color(255, 255, 255, 50), 1),
            BorderFactory.createEmptyBorder(15, 25, 15, 25)
        ));
        btn.setFocusPainted(false);
        btn.setCursor(new Cursor(Cursor.HAND_CURSOR));
        btn.setMaximumSize(new Dimension(200, 50));
        btn.setAlignmentX(Component.CENTER_ALIGNMENT);
        
        btn.addActionListener(e -> {
            cardLayout.show(mainPanel, card);
            animateButton(btn);
        });
        
        btn.addMouseListener(new MouseAdapter() {
            public void mouseEntered(MouseEvent e) {
                btn.setBackground(new Color(255, 255, 255, 50));
            }
            public void mouseExited(MouseEvent e) {
                btn.setBackground(new Color(255, 255, 255, 30));
            }
        });
        
        return btn;
    }
    
    private void animateButton(JButton btn) {
        javax.swing.Timer timer = new javax.swing.Timer(50, null);
        final float[] scale = {1.0f};
        timer.addActionListener(e -> {
            scale[0] += 0.02f;
            if (scale[0] >= 1.1f) {
                scale[0] = 1.0f;
                timer.stop();
            }
        });
        timer.start();
    }
    
    private JPanel createDashboardPanel() {
        JPanel panel = new JPanel(new GridLayout(2, 2, 20, 20));
        panel.setOpaque(false);
        panel.setBorder(new EmptyBorder(20, 20, 20, 20));
        
        panel.add(createStatCard("Total Rooms", "10", primaryColor));
        panel.add(createStatCard("Available", String.valueOf(rooms.stream().filter(r -> r.status.equals("Available")).count()), successColor));
        panel.add(createStatCard("Occupied", String.valueOf(rooms.stream().filter(r -> r.status.equals("Occupied")).count()), accentColor));
        panel.add(createStatCard("Revenue Today", "$1,250", secondaryColor));
        
        return panel;
    }
    
    private JPanel createStatCard(String title, String value, Color color) {
        JPanel card = new JPanel();
        card.setLayout(new BoxLayout(card, BoxLayout.Y_AXIS));
        card.setBackground(new Color(255, 255, 255, 240));
        card.setBorder(BorderFactory.createCompoundBorder(
            BorderFactory.createLineBorder(color, 2),
            BorderFactory.createEmptyBorder(30, 30, 30, 30)
        ));
        
        JLabel titleLabel = new JLabel(title);
        titleLabel.setFont(new Font("Segoe UI", Font.PLAIN, 18));
        titleLabel.setForeground(Color.DARK_GRAY);
        titleLabel.setAlignmentX(Component.CENTER_ALIGNMENT);
        
        JLabel valueLabel = new JLabel(value);
        valueLabel.setFont(new Font("Segoe UI", Font.BOLD, 48));
        valueLabel.setForeground(color);
        valueLabel.setAlignmentX(Component.CENTER_ALIGNMENT);
        
        card.add(titleLabel);
        card.add(Box.createVerticalStrut(10));
        card.add(valueLabel);
        
        return card;
    }
    
    private JPanel createRoomsPanel() {
        JPanel panel = new JPanel(new BorderLayout(10, 10));
        panel.setOpaque(false);
        panel.setBorder(new EmptyBorder(20, 20, 20, 20));
        
        JPanel header = new JPanel(new FlowLayout(FlowLayout.LEFT));
        header.setOpaque(false);
        
        JLabel title = new JLabel("Room Management");
        title.setFont(new Font("Segoe UI", Font.BOLD, 28));
        title.setForeground(Color.WHITE);
        header.add(title);
        
        panel.add(header, BorderLayout.NORTH);
        
        String[] columns = {"Room #", "Type", "Price/Night", "Status", "Actions"};
        tableModel = new DefaultTableModel(columns, 0) {
            public boolean isCellEditable(int row, int column) {
                return column == 4;
            }
        };
        
        refreshTableData();
        
        roomsTable = new JTable(tableModel);
        roomsTable.setRowHeight(40);
        roomsTable.setFont(new Font("Segoe UI", Font.PLAIN, 14));
        roomsTable.setBackground(new Color(255, 255, 255, 240));
        roomsTable.getTableHeader().setFont(new Font("Segoe UI", Font.BOLD, 14));
        roomsTable.getTableHeader().setBackground(primaryColor);
        roomsTable.getTableHeader().setForeground(Color.WHITE);
        
        JScrollPane scrollPane = new JScrollPane(roomsTable);
        scrollPane.setOpaque(false);
        scrollPane.getViewport().setOpaque(false);
        
        panel.add(scrollPane, BorderLayout.CENTER);
        
        JPanel buttonPanel = new JPanel(new FlowLayout(FlowLayout.RIGHT));
        buttonPanel.setOpaque(false);
        
        JButton refreshBtn = createActionButton("Refresh", primaryColor);
        refreshBtn.addActionListener(e -> refreshTableData());
        buttonPanel.add(refreshBtn);
        
        panel.add(buttonPanel, BorderLayout.SOUTH);
        
        return panel;
    }
    
    private void refreshTableData() {
        tableModel.setRowCount(0);
        for (Room room : rooms) {
            Object[] row = {room.number, room.type, "$" + room.price, room.status, "Book"};
            tableModel.addRow(row);
        }
    }
    
    private JPanel createBookingsPanel() {
        JPanel panel = new JPanel(new BorderLayout());
        panel.setOpaque(false);
        panel.setBorder(new EmptyBorder(20, 20, 20, 20));
        
        JLabel title = new JLabel("Recent Bookings", SwingConstants.CENTER);
        title.setFont(new Font("Segoe UI", Font.BOLD, 28));
        title.setForeground(Color.WHITE);
        panel.add(title, BorderLayout.NORTH);
        
        String[][] data = {
            {"John Smith", "Suite 101", "Jun 15-20, 2026", "$1,250", "Confirmed"},
            {"Sarah Johnson", "Double 102", "Jun 16-18, 2026", "$300", "Confirmed"},
            {"Mike Davis", "Single 103", "Jun 17-19, 2026", "$200", "Pending"}
        };
        
        String[] columns = {"Guest", "Room", "Dates", "Total", "Status"};
        JTable bookingsTable = new JTable(data, columns);
        bookingsTable.setRowHeight(35);
        bookingsTable.setFont(new Font("Segoe UI", Font.PLAIN, 14));
        bookingsTable.setBackground(new Color(255, 255, 255, 240));
        
        panel.add(new JScrollPane(bookingsTable), BorderLayout.CENTER);
        
        return panel;
    }
    
    private JPanel createGuestsPanel() {
        JPanel panel = new JPanel(new BorderLayout());
        panel.setOpaque(false);
        panel.setBorder(new EmptyBorder(20, 20, 20, 20));
        
        JLabel title = new JLabel("Guest Management", SwingConstants.CENTER);
        title.setFont(new Font("Segoe UI", Font.BOLD, 28));
        title.setForeground(Color.WHITE);
        panel.add(title, BorderLayout.NORTH);
        
        JPanel content = new JPanel(new GridLayout(2, 2, 20, 20));
        content.setOpaque(false);
        
        content.add(createGuestCard("John Smith", "New York", "Gold Member"));
        content.add(createGuestCard("Sarah Johnson", "Los Angeles", "Platinum Member"));
        content.add(createGuestCard("Mike Davis", "Chicago", "Silver Member"));
        content.add(createGuestCard("Emma Wilson", "Boston", "Gold Member"));
        
        panel.add(content, BorderLayout.CENTER);
        
        return panel;
    }
    
    private JPanel createGuestCard(String name, String location, String membership) {
        JPanel card = new JPanel();
        card.setLayout(new BoxLayout(card, BoxLayout.Y_AXIS));
        card.setBackground(new Color(255, 255, 255, 240));
        card.setBorder(BorderFactory.createCompoundBorder(
            BorderFactory.createLineBorder(primaryColor, 2),
            BorderFactory.createEmptyBorder(20, 20, 20, 20)
        ));
        
        JLabel nameLabel = new JLabel(name);
        nameLabel.setFont(new Font("Segoe UI", Font.BOLD, 20));
        nameLabel.setAlignmentX(Component.CENTER_ALIGNMENT);
        
        JLabel locLabel = new JLabel(location);
        locLabel.setFont(new Font("Segoe UI", Font.PLAIN, 14));
        locLabel.setAlignmentX(Component.CENTER_ALIGNMENT);
        
        JLabel memberLabel = new JLabel(membership);
        memberLabel.setFont(new Font("Segoe UI", Font.BOLD, 14));
        memberLabel.setForeground(secondaryColor);
        memberLabel.setAlignmentX(Component.CENTER_ALIGNMENT);
        
        card.add(nameLabel);
        card.add(Box.createVerticalStrut(10));
        card.add(locLabel);
        card.add(Box.createVerticalStrut(5));
        card.add(memberLabel);
        
        return card;
    }
    
    private JButton createActionButton(String text, Color color) {
        JButton btn = new JButton(text);
        btn.setFont(new Font("Segoe UI", Font.BOLD, 14));
        btn.setForeground(Color.WHITE);
        btn.setBackground(color);
        btn.setBorder(BorderFactory.createEmptyBorder(10, 20, 10, 20));
        btn.setFocusPainted(false);
        btn.setCursor(new Cursor(Cursor.HAND_CURSOR));
        return btn;
    }
    
    public static void main(String[] args) {
        SwingUtilities.invokeLater(() -> {
            try {
                UIManager.setLookAndFeel(UIManager.getSystemLookAndFeelClassName());
            } catch (Exception e) {
                e.printStackTrace();
            }
            new HotelManagement().setVisible(true);
        });
    }
}

class Room {
    int number;
    String type;
    double price;
    String status;
    
    Room(int number, String type, double price, String status) {
        this.number = number;
        this.type = type;
        this.price = price;
        this.status = status;
    }
}

class GradientPanel extends JPanel {
    @Override
    protected void paintComponent(Graphics g) {
        super.paintComponent(g);
        Graphics2D g2d = (Graphics2D) g;
        g2d.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON);
        
        GradientPaint gp = new GradientPaint(
            0, 0, new Color(102, 126, 234),
            getWidth(), getHeight(), new Color(118, 75, 162)
        );
        
        g2d.setPaint(gp);
        g2d.fillRect(0, 0, getWidth(), getHeight());
    }
}
